//! Only a complete integrity-checked resource set can become executable.
use lexift_core::{
    Error, Result,
    ports::ocr::{DownloadProgress, OcrResourceEntry, OcrResourceStatus, OcrResources},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Manifest {
    version: String,
    url: String,
    bytes: u64,
    sha256: String,
    files: Vec<Entry>,
    #[serde(default)]
    compatible_base: String,
}
#[derive(Deserialize)]
struct Entry {
    name: String,
    bytes: u64,
    sha256: String,
}
fn manifest() -> Manifest {
    serde_json::from_str(include_str!("assets.json")).expect("build-time OCR manifest")
}
fn korean_manifest() -> Manifest {
    serde_json::from_str(include_str!("korean-assets.json")).expect("build-time Korean manifest")
}
fn error(e: impl std::fmt::Display) -> Error {
    Error::new(format!("OCR resources: {e}"))
}
fn digest(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(error)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer).map_err(error)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn check(folder: &Path, m: &Manifest) -> Result<()> {
    for e in &m.files {
        if Path::new(&e.name).components().count() != 1 {
            return Err(error("invalid file name"));
        }
        let path = folder.join(&e.name);
        if fs::metadata(&path).map_err(error)?.len() != e.bytes || digest(&path)? != e.sha256 {
            return Err(error("integrity check failed"));
        }
    }
    Ok(())
}
pub(super) fn verified_directory(root: &Path) -> Result<PathBuf> {
    let m = manifest();
    if root.join("pending-removal").exists() {
        return Err(error("OCR resources will be removed after restart"));
    }
    let folder = root.join(&m.version);
    check(&folder, &m)?;
    Ok(folder)
}
/// Inspect the known bundle only; symlinks and junctions are never followed for deletion.
fn bundle(root: &Path, version: &str) -> Result<PathBuf> {
    if Path::new(version).components().count() != 1 || version == "." || version == ".." {
        return Err(error("invalid resource version"));
    }
    let folder = root.join(version);
    for path in [root, &folder] {
        if path.exists() {
            let metadata = fs::symlink_metadata(path).map_err(error)?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(error("resource directory is a reparse point"));
                }
            }
            if metadata.file_type().is_symlink() {
                return Err(error("resource directory is a link"));
            }
        }
    }
    Ok(folder)
}
pub(super) fn status(root: &Path) -> Result<OcrResources> {
    let mut base = status_for(root, &manifest())?;
    let korean = korean_manifest();
    let mut extension = status_for(root, &korean)?;
    if korean.compatible_base != base.version {
        extension.ready = false;
        extension.status = OcrResourceStatus::Damaged;
    }
    base.korean = Some(Box::new(extension));
    Ok(base)
}
fn status_for(root: &Path, m: &Manifest) -> Result<OcrResources> {
    let folder = bundle(root, &m.version)?;
    let pending = root.join("pending-removal").exists();
    let ready = !pending && check(&folder, m).is_ok();
    let entries: Vec<OcrResourceEntry> = m
        .files
        .iter()
        .map(|e| OcrResourceEntry {
            id: e.name.clone(),
            bytes: e.bytes,
        })
        .collect();
    let installed_bytes = m
        .files
        .iter()
        .filter_map(|e| fs::symlink_metadata(folder.join(&e.name)).ok())
        .filter(|m| m.is_file() && !m.file_type().is_symlink())
        .map(|m| m.len())
        .sum();
    Ok(OcrResources {
        ready,
        download_bytes: m.bytes,
        version: m.version.clone(),
        installed_bytes,
        entries,
        korean: None,
        status: if pending {
            OcrResourceStatus::PendingRemoval
        } else if ready {
            OcrResourceStatus::Ready
        } else if folder.exists() {
            OcrResourceStatus::Damaged
        } else {
            OcrResourceStatus::NotInstalled
        },
    })
}
/// Startup calls this before an inference environment can load the runtime DLL.
pub(super) fn cleanup_pending(root: &Path) -> Result<()> {
    let marker = root.join("pending-removal");
    if !marker.exists() {
        return Ok(());
    }
    let version = fs::read_to_string(&marker).map_err(error)?;
    if ![manifest().version.as_str(), "v2", "v3"].contains(&version.as_str()) {
        return Err(error("unknown pending resource version"));
    }
    let folder = bundle(root, &version)?;
    if folder.exists() {
        fs::remove_dir_all(folder).map_err(error)?;
    }
    remove_korean(root)?;
    fs::remove_file(marker).map_err(error)
}
pub(super) fn remove(root: &Path, runtime_loaded: bool) -> Result<OcrResources> {
    let m = manifest();
    let _ = bundle(root, &m.version)?;
    fs::create_dir_all(root).map_err(error)?;
    let marker = root.join("pending-removal");
    if !marker.exists() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(error)?
            .as_nanos();
        let temporary = root.join(format!(
            "pending-removal-{}-{nonce}.tmp",
            std::process::id()
        ));
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(error)?;
        output.write_all(m.version.as_bytes()).map_err(error)?;
        output.sync_all().map_err(error)?;
        drop(output);
        if let Err(cause) = fs::rename(&temporary, &marker) {
            let _ = fs::remove_file(&temporary);
            return Err(error(cause));
        }
    }
    if !runtime_loaded {
        cleanup_pending(root)?;
    }
    status(root)
}
pub(super) fn remove_korean(root: &Path) -> Result<()> {
    let folder = bundle(root, &korean_manifest().version)?;
    if folder.exists() {
        fs::remove_dir_all(folder).map_err(error)?;
    }
    Ok(())
}
pub(super) fn download_korean(
    root: &Path,
    cancel: &AtomicBool,
    progress: DownloadProgress,
) -> Result<()> {
    verified_directory(root)?;
    if korean_manifest().compatible_base != manifest().version {
        return Err(error("incompatible Korean extension"));
    }
    download_from(root, &korean_manifest(), cancel, progress)
}
/// Metadata identities invalidate the in-process verification cache without rereading model bodies.
pub(super) fn fingerprint(
    root: &Path,
) -> Result<Vec<(PathBuf, u64, Option<std::time::SystemTime>)>> {
    let mut stamp = Vec::new();
    for m in [manifest(), korean_manifest()] {
        let folder = bundle(root, &m.version)?;
        for name in m
            .files
            .iter()
            .map(|e| e.name.as_str())
            .chain(std::iter::once(""))
        {
            let path = folder.join(name);
            match fs::symlink_metadata(&path) {
                Ok(meta) if !meta.file_type().is_symlink() => {
                    #[cfg(windows)]
                    {
                        use std::os::windows::fs::MetadataExt;
                        if meta.file_attributes() & 0x400 != 0 {
                            return Err(error("resource is a reparse point"));
                        }
                    }
                    stamp.push((path, meta.len(), meta.modified().ok()));
                }
                Ok(_) => return Err(error("resource is a link")),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => stamp.push((path, 0, None)),
                Err(e) => return Err(error(e)),
            }
        }
    }
    let marker = root.join("pending-removal");
    if let Ok(meta) = fs::symlink_metadata(&marker) {
        stamp.push((marker, meta.len(), meta.modified().ok()));
    }
    Ok(stamp)
}
pub(super) fn download(root: &Path, cancel: &AtomicBool, progress: DownloadProgress) -> Result<()> {
    download_from(root, &manifest(), cancel, progress)
}
fn download_from(
    root: &Path,
    m: &Manifest,
    cancel: &AtomicBool,
    progress: DownloadProgress,
) -> Result<()> {
    if root.join("pending-removal").exists() {
        return Err(error("OCR resources will be removed after restart"));
    }
    let _ = bundle(root, &m.version)?;
    fs::create_dir_all(root).map_err(error)?;
    let temporary = root.join("download.tmp");
    let stage = root.join("staging");
    let result = (|| {
        let received = fetch(&temporary, m, cancel, &progress)?;
        if received != m.bytes || digest(&temporary)? != m.sha256 {
            return Err(error("download integrity check failed"));
        }
        if stage.exists() {
            fs::remove_dir_all(&stage).map_err(error)?;
        }
        fs::create_dir(&stage).map_err(error)?;
        let mut archive =
            zip::ZipArchive::new(fs::File::open(&temporary).map_err(error)?).map_err(error)?;
        for e in &m.files {
            if cancel.load(Ordering::Acquire) {
                return Err(error("download cancelled"));
            }
            if Path::new(&e.name).components().count() != 1 {
                return Err(error("invalid file name"));
            }
            let mut item = archive.by_name(&e.name).map_err(error)?;
            if item.size() != e.bytes {
                return Err(error("archive size mismatch"));
            }
            let mut output = fs::File::create(stage.join(&e.name)).map_err(error)?;
            std::io::copy(&mut item, &mut output).map_err(error)?;
            output.sync_all().map_err(error)?;
        }
        check(&stage, m)?;
        if cancel.load(Ordering::Acquire) {
            return Err(error("download cancelled"));
        }
        let destination = root.join(&m.version);
        if destination.exists() {
            if check(&destination, m).is_ok() {
                return Ok(());
            }
            let quarantine = root.join(format!(
                "corrupt-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            fs::rename(&destination, quarantine).map_err(error)?;
        }
        fs::rename(&stage, destination).map_err(error)?;
        progress(received, m.bytes);
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    if stage.exists() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

/// Dropping the network future cancels a stalled transfer as well as a running download.
fn fetch(
    path: &Path,
    m: &Manifest,
    cancel: &AtomicBool,
    progress: &DownloadProgress,
) -> Result<u64> {
    tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(error)?.block_on(async {
        let transfer=async {
            let client=reqwest::Client::builder().connect_timeout(Duration::from_secs(20)).timeout(Duration::from_secs(600)).build().map_err(error)?;
            let mut response=client.get(&m.url).send().await.map_err(error)?.error_for_status().map_err(error)?;
            if response.content_length().is_some_and(|n|n!=m.bytes){return Err(error("download size mismatch"));}
            let mut file=fs::File::create(path).map_err(error)?;let mut received=0;let mut last=Instant::now();
            while let Some(chunk)=response.chunk().await.map_err(error)? {
                received+=chunk.len() as u64;if received>m.bytes{return Err(error("download size mismatch"));}
                file.write_all(&chunk).map_err(error)?;
                if last.elapsed()>=Duration::from_millis(50){progress(received,m.bytes);last=Instant::now();}
            }
            file.sync_all().map_err(error)?;Ok(received)
        };
        let cancellation=async {loop {if cancel.load(Ordering::Acquire){break;}tokio::time::sleep(Duration::from_millis(25)).await;}};
        tokio::select! {biased; _=cancellation=>Err(error("download cancelled")), result=transfer=>result}
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, net::TcpListener, sync::Arc, thread};
    fn fixture(root: &Path) -> (Manifest, thread::JoinHandle<()>) {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("model.onnx", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"fixture model").unwrap();
        let archive = zip.finish().unwrap().into_inner();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let m = Manifest {
            compatible_base: String::new(),
            version: "fixture-v1".into(),
            url: format!("http://{}/bundle", listener.local_addr().unwrap()),
            bytes: archive.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&archive)),
            files: vec![Entry {
                name: "model.onnx".into(),
                bytes: 13,
                sha256: format!("{:x}", Sha256::digest(b"fixture model")),
            }],
        };
        fs::create_dir_all(root).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                archive.len()
            )
            .unwrap();
            stream.write_all(&archive).unwrap();
        });
        (m, server)
    }
    fn temporary() -> PathBuf {
        std::env::temp_dir().join(format!(
            "lexift-ocr-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn removal_is_scoped_and_loaded_runtime_leaves_durable_pending_marker() {
        let root = temporary();
        let version = manifest().version;
        fs::create_dir_all(root.join(&version)).unwrap();
        fs::write(root.join(&version).join("model.onnx"), b"fixture").unwrap();
        fs::write(root.join("unrelated"), b"keep").unwrap();
        assert_eq!(status(&root).unwrap().status, OcrResourceStatus::Damaged);
        let removed = remove(&root, true).unwrap();
        assert_eq!(removed.status, OcrResourceStatus::PendingRemoval);
        assert!(verified_directory(&root).is_err());
        assert!(download(&root, &AtomicBool::new(false), Arc::new(|_, _| {})).is_err());
        cleanup_pending(&root).unwrap();
        assert!(!root.join(version).exists());
        assert_eq!(fs::read(root.join("unrelated")).unwrap(), b"keep");
        assert_eq!(
            status(&root).unwrap().status,
            OcrResourceStatus::NotInstalled
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn pending_removal_preserves_unrelated_version_directory() {
        let root = temporary();
        fs::create_dir_all(root.join("v1")).unwrap();
        fs::create_dir_all(root.join("v2")).unwrap();
        fs::write(root.join("v1/old-model"), b"old").unwrap();
        fs::write(root.join("v2/new-model"), b"new").unwrap();
        fs::write(root.join("pending-removal"), "v1").unwrap();
        cleanup_pending(&root).unwrap();
        assert!(!root.join("v1").exists());
        assert_eq!(fs::read(root.join("v2/new-model")).unwrap(), b"new");
        assert!(!root.join("pending-removal").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_pending_version_cannot_escape_resource_root() {
        let root = temporary();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("pending-removal"), "../outside").unwrap();
        assert!(cleanup_pending(&root).is_err());
        assert!(root.join("pending-removal").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn failed_cleanup_preserves_pending_marker_and_can_retry_after_unlock() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = temporary();
        let folder = root.join(manifest().version);
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("locked.dll"), b"fixture").unwrap();
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(folder.join("locked.dll"))
            .unwrap();
        assert!(remove(&root, false).is_err());
        assert_eq!(
            status(&root).unwrap().status,
            OcrResourceStatus::PendingRemoval
        );
        drop(held);
        cleanup_pending(&root).unwrap();
        assert!(!folder.exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "requires verified OCR resources; runs in a separate process before cleanup test"]
    fn real_loaded_runtime_defers_complete_bundle_removal() {
        let source = PathBuf::from(
            std::env::var("LEXIFT_OCR_RESOURCE_FIXTURE").expect("verified source fixture"),
        );
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ocr-provider-removal-fixture");
        assert!(
            !root.exists(),
            "fixture must be new; do not delete user resources"
        );
        let m = manifest();
        check(&source, &m).unwrap();
        let folder = root.join(&m.version);
        fs::create_dir_all(&folder).unwrap();
        for e in &m.files {
            fs::copy(source.join(&e.name), folder.join(&e.name)).unwrap();
        }
        let engine = super::super::inference::Engine::new(&folder).unwrap();
        drop(engine);
        assert!(super::super::inference::RUNTIME_LOADED.load(Ordering::Acquire));
        let info = remove(&root, true).unwrap();
        assert_eq!(info.status, OcrResourceStatus::PendingRemoval);
        assert!(fs::remove_file(folder.join("onnxruntime.dll")).is_err());
        assert!(root.join("pending-removal").exists());
    }
    #[test]
    #[ignore = "run in a fresh process after real_loaded_runtime_defers_complete_bundle_removal"]
    fn real_runtime_startup_cleans_previous_process_pending_bundle() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/ocr-provider-removal-fixture");
        assert!(root.join("pending-removal").exists());
        cleanup_pending(&root).unwrap();
        assert_eq!(
            status(&root).unwrap().status,
            OcrResourceStatus::NotInstalled
        );
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn local_http_bundle_is_verified_then_atomically_enabled() {
        let root = temporary();
        let (m, server) = fixture(&root);
        let progress = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let p = progress.clone();
        download_from(
            &root,
            &m,
            &AtomicBool::new(false),
            Arc::new(move |n, _| {
                p.store(n, Ordering::Release);
            }),
        )
        .unwrap();
        server.join().unwrap();
        check(&root.join(&m.version), &m).unwrap();
        assert_eq!(progress.load(Ordering::Acquire), m.bytes);
        assert!(!root.join("download.tmp").exists());
        assert!(!root.join("staging").exists());
        fs::write(root.join(&m.version).join("model.onnx"), b"damaged").unwrap();
        assert!(check(&root.join(&m.version), &m).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_download_cannot_replace_a_previous_version() {
        let root = temporary();
        let (mut m, server) = fixture(&root);
        fs::create_dir(root.join("previous")).unwrap();
        fs::write(root.join("previous/model.onnx"), b"known-good").unwrap();
        m.sha256 = "invalid".into();
        assert!(download_from(&root, &m, &AtomicBool::new(false), Arc::new(|_, _| {})).is_err());
        server.join().unwrap();
        assert_eq!(
            fs::read(root.join("previous/model.onnx")).unwrap(),
            b"known-good"
        );
        assert!(!root.join(&m.version).exists());
        assert!(!root.join("download.tmp").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cancellation_interrupts_a_stalled_http_response() {
        let root = temporary();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/bundle", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_millis(400));
        });
        let m = Manifest {
            compatible_base: String::new(),
            version: "cancelled".into(),
            url,
            bytes: 1,
            sha256: String::new(),
            files: vec![],
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let timer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(80));
            c.store(true, Ordering::Release);
        });
        let start = Instant::now();
        assert!(download_from(&root, &m, &cancel, Arc::new(|_, _| {})).is_err());
        assert!(start.elapsed() < Duration::from_millis(350));
        timer.join().unwrap();
        server.join().unwrap();
        assert!(!root.join("cancelled").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unverified_runtime_is_rejected_before_engine_creation() {
        let root = temporary();
        fs::create_dir_all(&root).unwrap();
        let m = manifest();
        fs::create_dir(root.join(&m.version)).unwrap();
        fs::write(root.join(&m.version).join("onnxruntime.dll"), b"unverified").unwrap();
        assert!(verified_directory(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "requires scripts/build-ocr-resources.py output"]
    fn full_resource_archive_downloads_and_validates_from_local_http() {
        let archive = fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/ocr-resources-v1/lexift-ocr-v1-windows-x86_64.zip"),
        )
        .unwrap();
        let mut m = manifest();
        assert_eq!(archive.len() as u64, m.bytes);
        assert_eq!(format!("{:x}", Sha256::digest(&archive)), m.sha256);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        m.url = format!("http://{}/bundle", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                archive.len()
            )
            .unwrap();
            stream.write_all(&archive).unwrap();
        });
        let root = temporary();
        download_from(&root, &m, &AtomicBool::new(false), Arc::new(|_, _| {})).unwrap();
        server.join().unwrap();
        verified_directory(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn extension_removal_is_independent_and_missing_base_prevents_download() {
        let root = temporary();
        fs::create_dir_all(root.join("v1")).unwrap();
        fs::create_dir_all(root.join("korean-v1")).unwrap();
        fs::write(root.join("v1/keep"), b"base").unwrap();
        fs::write(root.join("korean-v1/model"), b"extension").unwrap();
        assert!(download_korean(&root, &AtomicBool::new(false), Arc::new(|_, _| {})).is_err());
        remove_korean(&root).unwrap();
        assert!(!root.join("korean-v1").exists());
        assert_eq!(fs::read(root.join("v1/keep")).unwrap(), b"base");
        fs::create_dir_all(root.join("korean-v1")).unwrap();
        remove(&root, false).unwrap();
        assert!(!root.join("v1").exists());
        assert!(!root.join("korean-v1").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[ignore = "requires generated release packages; local HTTP only"]
    fn real_split_packages_round_trip_http_and_independent_removal() {
        let root = temporary();
        let archives =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ocr-resources-v1");
        for mut m in [manifest(), korean_manifest()] {
            let data =
                fs::read(archives.join(format!("lexift-ocr-{}-windows-x86_64.zip", m.version)))
                    .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            m.url = format!("http://{}/bundle", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (mut client, _) = listener.accept().unwrap();
                let mut request = [0u8; 2048];
                assert!(client.read(&mut request).unwrap() > 0);
                write!(
                    client,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    data.len()
                )
                .unwrap();
                client.write_all(&data).unwrap();
            });
            download_from(&root, &m, &AtomicBool::new(false), Arc::new(|_, _| {})).unwrap();
            server.join().unwrap();
            check(&root.join(&m.version), &m).unwrap();
        }
        let info = status(&root).unwrap();
        assert!(info.ready);
        assert!(info.korean.unwrap().ready);
        remove_korean(&root).unwrap();
        let info = status(&root).unwrap();
        assert!(info.ready);
        assert!(!info.korean.unwrap().ready);
        remove(&root, false).unwrap();
        assert!(!status(&root).unwrap().ready);
        fs::remove_dir_all(root).unwrap();
    }
}

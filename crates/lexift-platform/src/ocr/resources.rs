//! Only a complete integrity-checked resource set can become executable.
use lexift_core::{
    Error, Result,
    ports::ocr::{DownloadProgress, OcrResources},
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
    let folder = root.join(&m.version);
    check(&folder, &m)?;
    Ok(folder)
}
pub(super) fn status(root: &Path) -> Result<OcrResources> {
    let m = manifest();
    Ok(OcrResources {
        ready: check(&root.join(&m.version), &m).is_ok(),
        download_bytes: m.bytes,
    })
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
                .join("../../target/ocr-resources/lexift-ocr-v1-windows-x86_64.zip"),
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
}

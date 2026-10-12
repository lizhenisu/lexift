//! Process-owned immutable font data; scanning never holds the service lock.
use fontdb::{Database, Family, Query};
use std::sync::{Arc, Mutex, OnceLock};

type Loader = dyn Fn() -> Result<Snapshot, &'static str> + Send + Sync;

#[derive(Clone)]
pub(super) struct Snapshot {
    pub db: Arc<Database>,
    pub names: Vec<String>,
    pub aliases: super::fonts::Aliases,
    pub sans: Option<String>,
}
impl Snapshot {
    fn scan() -> Result<Self, &'static str> {
        let mut db = Database::new();
        db.load_system_fonts();
        Self::from_database(db)
    }
    pub fn from_database(db: Database) -> Result<Self, &'static str> {
        let mut names = db
            .faces()
            .flat_map(|f| f.families.iter().map(|f| f.0.clone()))
            .collect();
        super::text::normalize_family_names(&mut names);
        if names.is_empty() {
            return Err("empty font directory");
        }
        let aliases = super::fonts::alias_index(db.faces().map(|f| f.families.clone()).collect());
        let sans = db
            .query(&Query {
                families: &[Family::SansSerif],
                ..Default::default()
            })
            .or_else(|| {
                [
                    "Noto Sans",
                    "DejaVu Sans",
                    "Liberation Sans",
                    "Helvetica Neue",
                    "Helvetica",
                    "Arial",
                ]
                .into_iter()
                .find_map(|family| {
                    db.query(&Query {
                        families: &[Family::Name(family)],
                        ..Default::default()
                    })
                })
            })
            .and_then(|id| db.face(id))
            .and_then(|f| f.families.first())
            .map(|f| f.0.clone());
        Ok(Self {
            db: Arc::new(db),
            names,
            aliases,
            sans,
        })
    }
}

#[derive(Clone)]
pub(super) enum Status {
    Unloaded,
    Loading(u64),
    Ready(u64, Arc<Snapshot>),
    Failed(u64),
}
impl Status {
    pub fn request(&self) -> u64 {
        match self {
            Self::Unloaded => 0,
            Self::Loading(id) | Self::Ready(id, _) | Self::Failed(id) => *id,
        }
    }
}
struct State {
    status: Status,
    next: u64,
}
pub(super) struct Service {
    state: Mutex<State>,
    loader: Arc<Loader>,
}
impl Service {
    pub fn new(loader: Arc<Loader>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                status: Status::Unloaded,
                next: 0,
            }),
            loader,
        })
    }
    pub fn status(&self) -> Status {
        self.state.lock().unwrap().status.clone()
    }
    pub fn start(
        self: &Arc<Self>,
        retry: bool,
        completed: impl FnOnce(u64) + Send + 'static,
    ) -> Status {
        let request = {
            let mut state = self.state.lock().unwrap();
            if !matches!(state.status, Status::Unloaded)
                && !(retry && matches!(state.status, Status::Failed(_)))
            {
                return state.status.clone();
            }
            state.next += 1;
            let request = state.next;
            state.status = Status::Loading(request);
            request
        };
        let service = self.clone();
        let result = std::thread::Builder::new()
            .name("annotation-fonts".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (service.loader)()))
                        .unwrap_or(Err("font loader panicked"));
                let ready = result.is_ok();
                {
                    let mut state = service.state.lock().unwrap();
                    if !matches!(state.status, Status::Loading(id) if id == request) {
                        return;
                    }
                    state.status = match result {
                        Ok(snapshot) => Status::Ready(request, Arc::new(snapshot)),
                        Err(_) => Status::Failed(request),
                    };
                }
                tracing::debug!(
                    request,
                    ready,
                    scan_ms = started.elapsed().as_secs_f64() * 1000.,
                    "Annotation font directory background preparation"
                );
                completed(request);
            });
        if result.is_err() {
            self.state.lock().unwrap().status = Status::Failed(request);
        }
        self.status()
    }
}

fn global() -> Arc<Service> {
    static SERVICE: OnceLock<Arc<Service>> = OnceLock::new();
    SERVICE
        .get_or_init(|| Service::new(Arc::new(Snapshot::scan)))
        .clone()
}

#[cfg(test)]
thread_local! { pub(super) static TEST_SERVICE: std::cell::RefCell<Option<Arc<Service>>> = const { std::cell::RefCell::new(None) }; }

pub(super) fn service() -> Arc<Service> {
    #[cfg(test)]
    {
        if let Some(service) = TEST_SERVICE.with(|s| s.borrow().clone()) {
            return service;
        }
        // Legacy pure raster tests have no event loop. Prepare their shared fixture
        // on a worker once; new loading tests inject a controllable service above.
        static FIXTURE: OnceLock<()> = OnceLock::new();
        FIXTURE.get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            let status = global().start(false, move |_| {
                let _ = tx.send(());
            });
            if matches!(status, Status::Loading(_)) {
                rx.recv().unwrap();
            }
        });
    }
    global()
}

pub(super) fn snapshot() -> Option<Arc<Snapshot>> {
    if let Status::Ready(_, snapshot) = service().status() {
        Some(snapshot)
    } else {
        None
    }
}
pub(super) fn ready() -> bool {
    snapshot().is_some()
}
pub(super) fn request(retry: bool) -> Status {
    service().start(retry, |request| {
        let _ = slint::invoke_from_event_loop(move || super::font_workflow::completed(request));
    })
}

pub(super) fn document_needs_fonts(session: &lexift_core::domain::annotation::Session) -> bool {
    session.watermark.is_some()
        || session.objects.iter().any(|o| {
            matches!(
                o.extra,
                lexift_core::domain::annotation::Extra::Text(_)
                    | lexift_core::domain::annotation::Extra::Sequence(_)
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    #[test]
    fn scan_is_nonblocking_coalesced_and_shared_with_raster_threads() {
        let fixture = snapshot().unwrap();
        let (release, wait) = mpsc::channel();
        let wait = Mutex::new(wait);
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let service = Service::new(Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            wait.lock().unwrap().recv().unwrap();
            Ok((*fixture).clone())
        }));
        let (done, completed) = mpsc::channel();
        let first = service.start(false, move |id| done.send(id).unwrap());
        let second = service.start(false, |_| panic!("duplicate worker"));
        assert!(matches!(first, Status::Loading(_)));
        assert_eq!(first.request(), second.request());
        assert!(
            completed.try_recv().is_err(),
            "request returns while worker is blocked"
        );
        release.send(()).unwrap();
        assert_eq!(
            completed
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap(),
            first.request()
        );
        let Status::Ready(_, loaded) = service.status() else {
            panic!("not ready")
        };
        let db = loaded.db.clone();
        let other = std::thread::spawn(move || loaded.db.clone())
            .join()
            .unwrap();
        assert!(Arc::ptr_eq(&db, &other));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(matches!(
            service.start(true, |_| panic!("ready must not reload")),
            Status::Ready(_, _)
        ));
    }

    #[test]
    fn failure_needs_explicit_retry_and_empty_directories_are_rejected() {
        assert!(Snapshot::from_database(Database::new()).is_err());
        let attempts = Arc::new(AtomicUsize::new(0));
        let calls = attempts.clone();
        let service = Service::new(Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err("unavailable")
        }));
        let (tx, rx) = mpsc::channel();
        service.start(false, move |_| tx.send(()).unwrap());
        rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        let old = service.status().request();
        assert!(matches!(
            service.start(false, |_| panic!("automatic retry")),
            Status::Failed(_)
        ));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        let (tx, rx) = mpsc::channel();
        service.start(true, move |_| tx.send(()).unwrap());
        rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert!(service.status().request() > old);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}

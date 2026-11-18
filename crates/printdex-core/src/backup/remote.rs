//! Remote backup targets (S3, WebDAV, SFTP) as a rustic backend over
//! OpenDAL.
//!
//! `rustic_backend`'s own OpenDAL backend compiles two dozen storage
//! services; this one is built with only the three Printdex offers. The
//! file layout is restic's (`config`, `keys/`, `snapshots/`, `index/`,
//! `data/<2 hex>/<id>`), adapted from rustic_backend 0.8
//! (`src/opendal.rs`, MIT OR Apache-2.0).

use std::sync::OnceLock;

use bytes::Bytes;
use opendal::blocking::Operator;
use opendal::layers::{RetryLayer, ThrottleLayer};
use opendal::options::{ListOptions, ReadOptions};
use rustic_core::{
    ALL_FILE_TYPES, BytesList, ErrorKind, FileType, Id, ReadBackend, RusticError, RusticResult,
    WriteBackend,
};
use tokio::runtime::Runtime;

/// Retries per request before an error counts.
const RETRIES: usize = 5;

/// The runtime OpenDAL's blocking API runs its futures on. Backups run on
/// blocking threads, never on the engine's async workers.
fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("printdex-backup-io")
            .enable_all()
            .build()
            .expect("backup I/O runtime")
    })
}

/// An upload limit in bytes per second (with a one-second burst).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Throttle(pub u32);

#[derive(Clone, Debug)]
pub struct RemoteBackend {
    op: Operator,
    location: String,
}

fn backend_err(what: &'static str, err: opendal::Error) -> Box<RusticError> {
    RusticError::with_source(ErrorKind::Backend, what, err)
}

impl RemoteBackend {
    /// Wraps an OpenDAL service builder.
    pub fn new(
        builder: impl opendal::Builder,
        location: String,
        throttle: Option<Throttle>,
    ) -> RusticResult<Self> {
        let _guard = runtime().enter();
        // Registers the reqwest HTTP transport (once per process).
        opendal::install_default();
        let mut op = opendal::Operator::new(builder)
            .map_err(|e| backend_err("Setting up the backup target failed.", e))?
            .layer(RetryLayer::new().with_max_times(RETRIES).with_jitter());
        if let Some(Throttle(bps)) = throttle {
            op = op.layer(ThrottleLayer::new(bps.max(1024), bps.max(1024)));
        }
        let op = Operator::new(op)
            .map_err(|e| backend_err("Setting up the backup target failed.", e))?;
        Ok(Self { op, location })
    }

    fn path(tpe: FileType, id: &Id) -> String {
        let hex = id.to_hex();
        match tpe {
            FileType::Config => "config".to_owned(),
            FileType::Pack => format!("data/{}/{}", &hex[0..2], &hex[..]),
            _ => format!("{}/{}", tpe.dirname(), &hex[..]),
        }
    }

    fn list_entries(&self, tpe: FileType) -> RusticResult<Vec<(Id, u64)>> {
        let opts = ListOptions {
            recursive: true,
            ..Default::default()
        };
        let lister = self
            .op
            .lister_options(&format!("{}/", tpe.dirname()), opts)
            .map_err(|e| backend_err("Listing the backup target failed.", e))?;
        let mut out = Vec::new();
        for entry in lister {
            let entry = entry.map_err(|e| backend_err("Listing the backup target failed.", e))?;
            if !entry.metadata().is_file() {
                continue;
            }
            if let Some(id) = Id::parse_some(entry.name(), tpe) {
                out.push((id, entry.metadata().content_length()));
            }
        }
        Ok(out)
    }
}

impl ReadBackend for RemoteBackend {
    fn location(&self) -> String {
        self.location.clone()
    }

    fn list(&self, tpe: FileType) -> RusticResult<Vec<Id>> {
        if tpe == FileType::Config {
            return Ok(if self.config_exists()? {
                vec![Id::default()]
            } else {
                Vec::new()
            });
        }
        Ok(self
            .list_entries(tpe)?
            .into_iter()
            .map(|(id, _)| id)
            .collect())
    }

    fn list_with_size(&self, tpe: FileType) -> RusticResult<Vec<(Id, u32)>> {
        if tpe == FileType::Config {
            return match self.op.stat("config") {
                Ok(meta) => Ok(vec![(
                    Id::default(),
                    u32::try_from(meta.content_length()).unwrap_or(u32::MAX),
                )]),
                Err(e) if e.kind() == opendal::ErrorKind::NotFound => Ok(Vec::new()),
                Err(e) => Err(backend_err("Reading the backup target failed.", e)),
            };
        }
        Ok(self
            .list_entries(tpe)?
            .into_iter()
            .filter_map(|(id, len)| Some((id, u32::try_from(len).ok()?)))
            .collect())
    }

    fn read_full(&self, tpe: FileType, id: &Id) -> RusticResult<Bytes> {
        Ok(self
            .op
            .read(&Self::path(tpe, id))
            .map_err(|e| backend_err("Reading the backup target failed.", e))?
            .to_bytes())
    }

    fn read_partial(
        &self,
        tpe: FileType,
        id: &Id,
        _cacheable: bool,
        offset: u32,
        length: u32,
    ) -> RusticResult<Bytes> {
        let opts = ReadOptions {
            range: (u64::from(offset)..u64::from(offset) + u64::from(length)).into(),
            ..Default::default()
        };
        Ok(self
            .op
            .read_options(&Self::path(tpe, id), opts)
            .map_err(|e| backend_err("Reading the backup target failed.", e))?
            .to_bytes())
    }

    fn warmup_path(&self, tpe: FileType, id: &Id) -> String {
        Self::path(tpe, id)
    }
}

impl RemoteBackend {
    fn config_exists(&self) -> RusticResult<bool> {
        match self.op.exists("config") {
            Ok(e) => Ok(e),
            Err(e) if e.kind() == opendal::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(backend_err("Reading the backup target failed.", e)),
        }
    }
}

impl WriteBackend for RemoteBackend {
    fn create(&self) -> RusticResult<()> {
        let mk = |p: String| {
            self.op
                .create_dir(&p)
                .map_err(|e| backend_err("Creating the backup repository failed.", e))
        };
        for tpe in ALL_FILE_TYPES {
            mk(format!("{}/", tpe.dirname()))?;
        }
        for i in 0u8..=255 {
            mk(format!("data/{i:02x}/"))?;
        }
        Ok(())
    }

    fn write_bytes(
        &self,
        tpe: FileType,
        id: &Id,
        _cacheable: bool,
        content: BytesList,
    ) -> RusticResult<()> {
        self.op
            .write(&Self::path(tpe, id), content.into_vec())
            .map_err(|e| backend_err("Writing to the backup target failed.", e))?;
        Ok(())
    }

    fn remove(&self, tpe: FileType, id: &Id, _cacheable: bool) -> RusticResult<()> {
        self.op
            .delete(&Self::path(tpe, id))
            .map_err(|e| backend_err("Deleting from the backup target failed.", e))
    }
}

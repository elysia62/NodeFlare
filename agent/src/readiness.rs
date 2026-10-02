use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// A startup marker only, not a heartbeat or a credential. Installers also
/// verify this PID belongs to the managed Agent and survives a short check.
pub(crate) struct ReadyMarker {
    path: PathBuf,
    pid: String,
    published: bool,
}

impl ReadyMarker {
    pub(crate) fn prepare(path: PathBuf) -> io::Result<Self> {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(Self {
            path,
            pid: format!("{}\n", std::process::id()),
            published: false,
        })
    }

    pub(crate) fn publish(&mut self) -> io::Result<()> {
        if self.published {
            return Ok(());
        }
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = self
            .path
            .with_file_name(format!(".ready.{}.{nonce}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        let result = file.write_all(self.pid.as_bytes());
        drop(file);
        let result = result.and_then(|()| fs::rename(&temporary, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        self.published = true;
        Ok(())
    }
}

impl Drop for ReadyMarker {
    fn drop(&mut self) {
        if self.published && fs::read_to_string(&self.path).is_ok_and(|pid| pid == self.pid) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("nodeflare-ready-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn readiness_is_atomic_one_time_and_clears_stale_startup_state() {
        let directory = directory();
        let path = directory.join("ready.pid");
        fs::write(&path, "123\n").unwrap();
        let mut marker = ReadyMarker::prepare(path.clone()).unwrap();
        assert!(!path.exists());
        marker.publish().unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("{}\n", std::process::id())
        );
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // Subsequent samples do not rewrite it or add disk I/O.
        fs::write(&path, "456\n").unwrap();
        marker.publish().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "456\n");
        drop(marker);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "456\n",
            "do not delete another instance's marker"
        );
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn readiness_cleans_up_its_own_marker_when_the_run_finishes() {
        let directory = directory();
        let path = directory.join("ready.pid");
        let mut marker = ReadyMarker::prepare(path.clone()).unwrap();
        marker.publish().unwrap();
        drop(marker);
        assert!(!path.exists());
        fs::remove_dir(directory).unwrap();
    }
}

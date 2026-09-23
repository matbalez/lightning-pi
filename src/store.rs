use crate::protocol::{NETWORK, SKEW};
use rusqlite::{Connection, params};
use std::{path::Path, sync::Mutex, time::Duration};

pub struct ReplayStore(Mutex<Connection>);
impl ReplayStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS settlements (consumption_key TEXT PRIMARY KEY, retain_until INTEGER NOT NULL) STRICT;")?;
        Ok(Self(Mutex::new(conn)))
    }
    pub fn consume(&self, payment_hash: &str, invoice_end: u64, now: u64) -> anyhow::Result<bool> {
        let mut conn = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Replay store lock poisoned"))?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM settlements WHERE retain_until < ?1", [now])?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO settlements(consumption_key,retain_until) VALUES (?1,?2)",
            params![
                format!("{NETWORK}:{payment_hash}"),
                invoice_end + SKEW + 3600
            ],
        )?;
        tx.commit()?;
        Ok(inserted == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durable_and_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("replay.sqlite");
        let a = ReplayStore::open(&path).unwrap();
        assert!(a.consume("abc", 5000, 100).unwrap());
        drop(a);
        let b = ReplayStore::open(&path).unwrap();
        assert!(!b.consume("abc", 5000, 100).unwrap());
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let p = path.clone();
                std::thread::spawn(move || {
                    ReplayStore::open(&p)
                        .unwrap()
                        .consume("racing", 5000, 100)
                        .unwrap()
                })
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .filter_map(|h| h.join().ok())
                .filter(|x| *x)
                .count(),
            1
        );
    }
}

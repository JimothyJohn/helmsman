//! A sink's bounded on-disk queue: what is waiting to be delivered,
//! surviving a restart.
//!
//! # File format
//!
//! A 4-byte magic `HSQ1`, then records, each
//!
//! ```text
//! len:u32 LE   crc:u32 LE (CRC-32/IEEE of the payload)   payload[len]
//! payload = 1:u8 id:u64 LE key_len:u16 LE key body     a pushed event
//!         | 2:u8 id:u64 LE                             event `id` is gone
//! ```
//!
//! Pushes append; delivering, rejecting or dropping an event appends a
//! pop. Loading replays the records in order and stops at the first one
//! that is short, too long, fails its CRC or does not decode, then cuts the
//! file back to the last good record: a half-written append (power cut
//! mid-write) is never replayed, and nothing after damage is trusted. A
//! file with a bad magic is renamed to `*.corrupt` and the queue starts
//! empty. The file is rewritten (to a temporary, then renamed) once it
//! grows past twice the queue's limit, and truncated whenever the queue
//! empties.
//!
//! # Bound
//!
//! `max_bytes` bounds the live events ([`cost`] each). When a push would
//! pass it, the *oldest* events are dropped and counted: during a long
//! outage the receiver gets the most recent state, which is what a
//! dashboard or MES acts on, rather than a stale morning followed by a
//! gap until now. The drop is visible in the event keys' `seq` gaps. An
//! event bigger than the whole bound is refused (and counted).
//!
//! Durability: [`Queue::sync`] after a batch of pushes; pops are not
//! synced, since a lost pop only means a resend, which at-least-once
//! delivery allows.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"HSQ1";
const PUSH: u8 = 1;
const POP: u8 = 2;
/// Length and CRC.
const FRAME: usize = 8;
/// Largest record payload accepted on load; bigger is damage.
pub const MAX_RECORD: usize = 1 << 20;
/// Kind, id and key length.
const PUSH_HEAD: usize = 1 + 8 + 2;
/// Largest idempotency key.
pub const MAX_KEY: usize = 256;

/// One queued event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub key: String,
    pub body: Vec<u8>,
}

/// What an entry costs against `max_bytes`: its record on disk.
#[must_use]
pub fn cost(key: &str, body: &[u8]) -> usize {
    FRAME + PUSH_HEAD + key.len() + body.len()
}

/// What loading found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovery {
    /// Bytes cut from the end of the file: a torn or damaged tail.
    pub cut_bytes: u64,
    /// The file had no valid magic and was moved aside to this path.
    pub moved_aside: Option<PathBuf>,
    /// Events dropped at load because the bound is now smaller.
    pub dropped: u64,
}

/// What a push did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pushed {
    /// Queued, after dropping this many of the oldest events.
    Queued { dropped: u64 },
    /// Bigger than the whole bound (or its key too long); not queued.
    TooBig,
}

pub struct Queue {
    path: PathBuf,
    file: File,
    file_len: u64,
    entries: VecDeque<Entry>,
    live: usize,
    max_bytes: usize,
    next_id: u64,
    dropped: u64,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("path", &self.path)
            .field("len", &self.entries.len())
            .field("bytes", &self.live)
            .field("max_bytes", &self.max_bytes)
            .finish_non_exhaustive()
    }
}

/// CRC-32/IEEE (reflected, poly 0xEDB88320), table-driven.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            #[allow(clippy::cast_possible_truncation)]
            let mut c = i as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 == 1 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            t[i] = c;
            i += 1;
        }
        t
    };
    let mut c = !0u32;
    for &b in data {
        c = TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

fn record(payload: &[u8]) -> Vec<u8> {
    let mut r = Vec::with_capacity(FRAME + payload.len());
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    r.extend_from_slice(&len.to_le_bytes());
    r.extend_from_slice(&crc32(payload).to_le_bytes());
    r.extend_from_slice(payload);
    r
}

fn push_record(e: &Entry) -> Vec<u8> {
    let mut p = Vec::with_capacity(PUSH_HEAD + e.key.len() + e.body.len());
    p.push(PUSH);
    p.extend_from_slice(&e.id.to_le_bytes());
    let klen = u16::try_from(e.key.len()).unwrap_or(u16::MAX);
    p.extend_from_slice(&klen.to_le_bytes());
    p.extend_from_slice(e.key.as_bytes());
    p.extend_from_slice(&e.body);
    record(&p)
}

fn pop_record(id: u64) -> Vec<u8> {
    let mut p = Vec::with_capacity(9);
    p.push(POP);
    p.extend_from_slice(&id.to_le_bytes());
    record(&p)
}

enum Rec {
    Push(Entry),
    Pop(u64),
}

fn decode(p: &[u8]) -> Option<Rec> {
    let (&kind, rest) = p.split_first()?;
    let id = u64::from_le_bytes(rest.get(..8)?.try_into().ok()?);
    let rest = &rest[8..];
    match kind {
        POP if rest.is_empty() => Some(Rec::Pop(id)),
        PUSH => {
            let klen = usize::from(u16::from_le_bytes(rest.get(..2)?.try_into().ok()?));
            let key = std::str::from_utf8(rest.get(2..2 + klen)?).ok()?.to_string();
            let body = rest[2 + klen..].to_vec();
            Some(Rec::Push(Entry { id, key, body }))
        }
        _ => None,
    }
}

/// Replays `data` (after the magic); returns the entries, the next id and
/// how many bytes of `data` were good.
fn replay(data: &[u8]) -> (VecDeque<Entry>, u64, usize) {
    let mut entries: VecDeque<Entry> = VecDeque::new();
    let mut next_id = 0u64;
    let mut at = 0;
    while let Some(frame) = data.get(at..at + FRAME) {
        let len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        let crc = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
        if len > MAX_RECORD {
            break;
        }
        let Some(payload) = data.get(at + FRAME..at + FRAME + len) else {
            break;
        };
        if crc32(payload) != crc {
            break;
        }
        match decode(payload) {
            Some(Rec::Push(e)) => {
                next_id = next_id.max(e.id.saturating_add(1));
                entries.push_back(e);
            }
            Some(Rec::Pop(id)) => {
                if let Some(i) = entries.iter().position(|e| e.id == id) {
                    entries.remove(i);
                }
            }
            None => break,
        }
        at += FRAME + len;
    }
    (entries, next_id, at)
}

impl Queue {
    /// Opens (or creates) the queue file at `path`, recovering what it can.
    ///
    /// # Errors
    /// The file cannot be read, written or created.
    pub fn open(path: &Path, max_bytes: usize) -> io::Result<(Self, Recovery)> {
        let mut rec = Recovery::default();
        let mut data = Vec::new();
        match File::open(path) {
            Ok(mut f) => {
                f.read_to_end(&mut data)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        // A file shorter than the magic but agreeing with it was cut while
        // being created: empty, not damaged.
        let torn_create = data.len() < MAGIC.len() && MAGIC.starts_with(&data);
        if !torn_create && !data.starts_with(MAGIC) {
            let aside = path.with_extension("corrupt");
            fs::rename(path, &aside)?;
            rec.moved_aside = Some(aside);
            data.clear();
        }
        let (entries, next_id, good) = if data.len() >= MAGIC.len() {
            replay(&data[MAGIC.len()..])
        } else {
            (VecDeque::new(), 0, 0)
        };
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        let good_len = if data.len() >= MAGIC.len() {
            (MAGIC.len() + good) as u64
        } else {
            0
        };
        rec.cut_bytes = (data.len() as u64).saturating_sub(good_len);
        if good_len == 0 {
            file.set_len(0)?;
            file.write_all(MAGIC)?;
        } else {
            file.set_len(good_len)?;
        }
        file.seek(SeekFrom::End(0))?;
        file.sync_all()?;
        let file_len = file.stream_position()?;
        let live = entries.iter().map(|e| cost(&e.key, &e.body)).sum();
        let mut q = Self {
            path: path.to_path_buf(),
            file,
            file_len,
            entries,
            live,
            max_bytes,
            next_id,
            dropped: 0,
        };
        while q.live > q.max_bytes {
            q.pop()?;
            rec.dropped += 1;
        }
        q.dropped = rec.dropped;
        Ok((q, rec))
    }

    /// Appends an event, dropping the oldest to make room. Not synced: call
    /// [`Queue::sync`] after a batch.
    ///
    /// # Errors
    /// The append failed; the queue in memory is unchanged.
    pub fn push(&mut self, key: &str, body: &[u8]) -> io::Result<Pushed> {
        let c = cost(key, body);
        if c > self.max_bytes || key.len() > MAX_KEY || c - FRAME > MAX_RECORD {
            self.dropped += 1;
            return Ok(Pushed::TooBig);
        }
        let mut dropped = 0;
        while self.live + c > self.max_bytes {
            self.pop()?;
            dropped += 1;
        }
        self.dropped += dropped;
        let e = Entry {
            id: self.next_id,
            key: key.to_string(),
            body: body.to_vec(),
        };
        self.append(&push_record(&e))?;
        self.next_id += 1;
        self.live += c;
        self.entries.push_back(e);
        Ok(Pushed::Queued { dropped })
    }

    /// The oldest event, next to deliver.
    #[must_use]
    pub fn front(&self) -> Option<&Entry> {
        self.entries.front()
    }

    /// Removes the oldest event (delivered, rejected or dropped).
    ///
    /// # Errors
    /// The pop could not be recorded; the event stays queued.
    pub fn pop(&mut self) -> io::Result<()> {
        let Some(front) = self.entries.front() else {
            return Ok(());
        };
        let id = front.id;
        if self.entries.len() == 1 {
            self.file.set_len(MAGIC.len() as u64)?;
            self.file.seek(SeekFrom::End(0))?;
            self.file_len = MAGIC.len() as u64;
        } else {
            self.append(&pop_record(id))?;
        }
        if let Some(e) = self.entries.pop_front() {
            self.live -= cost(&e.key, &e.body);
        }
        if self.file_len > 2 * self.max_bytes as u64 + 4096 {
            self.compact()?;
        }
        Ok(())
    }

    /// Flushes appended records to the disk.
    ///
    /// # Errors
    /// What `fsync` returns.
    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_data()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes of live events.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.live
    }

    /// Events dropped to stay within the bound, or refused as too big,
    /// since this queue was opened.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The queue file's current size.
    #[must_use]
    pub fn file_len(&self) -> u64 {
        self.file_len
    }

    /// The queued events, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }

    fn append(&mut self, rec: &[u8]) -> io::Result<()> {
        if let Err(e) = self.file.write_all(rec) {
            // Cut a partial write so the next append starts on a boundary.
            let _ = self.file.set_len(self.file_len);
            let _ = self.file.seek(SeekFrom::Start(self.file_len));
            return Err(e);
        }
        self.file_len += rec.len() as u64;
        Ok(())
    }

    /// Rewrites the file with only the live events: to a temporary, synced,
    /// then renamed over the queue.
    fn compact(&mut self) -> io::Result<()> {
        let tmp = self.path.with_extension("tmp");
        let mut out = Vec::with_capacity(MAGIC.len() + self.live);
        out.extend_from_slice(MAGIC);
        for e in &self.entries {
            out.extend_from_slice(&push_record(e));
        }
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&out)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        if let Some(dir) = self.path.parent()
            && let Ok(d) = File::open(dir)
        {
            let _ = d.sync_all();
        }
        let mut file = OpenOptions::new().read(true).write(true).open(&self.path)?;
        file.seek(SeekFrom::End(0))?;
        self.file = file;
        self.file_len = out.len() as u64;
        Ok(())
    }
}

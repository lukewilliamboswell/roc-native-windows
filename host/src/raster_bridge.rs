//! Bind binary image inputs to the lifetime of mounted native nodes.

use crate::picture::Resize;
use crate::raster::{MAX_PIXELS, Pixels, byte_count, decode_asset};
use crate::wire::{ATTACH_RASTER_PORT, Key, Kind, Reader, Tree};
use crate::{Event, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, Weak};

const BIND: &str = "roc-raster-bind";
const MAX_QUEUED: usize = 64;
const MAX_QUEUED_BYTES: usize = 64 * 1024 * 1024;

type Picture = Option<Arc<Pixels>>;
pub(crate) type Update = (Key, Picture);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Job {
    Source(Key),
    Resize(Key),
}

impl Job {
    fn key(self) -> Key {
        match self {
            Self::Source(key) | Self::Resize(key) => key,
        }
    }
}

pub(crate) struct ResizeUpdate {
    pub(crate) key: Key,
    pub(crate) source: Arc<Pixels>,
    pub(crate) image: Arc<Pixels>,
}

enum Prepared {
    Source(Arc<Pixels>),
    Resize(Arc<Pixels>, Arc<Pixels>),
}

pub(crate) struct Attachment {
    key: Key,
    source: Source,
}

enum Source {
    Ready(Picture),
    Raw {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Asset {
        root: PathBuf,
        name: String,
    },
    Resize(Resize),
}

impl Source {
    fn queued_bytes(&self) -> usize {
        match self {
            Self::Raw { rgba, .. } => rgba.len(),
            Self::Resize(Resize {
                size: (width, height),
                ..
            }) => byte_count(*width, *height).unwrap_or(0),
            _ => 0,
        }
    }
}

impl Attachment {
    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self> {
        if reader.string()? != ATTACH_RASTER_PORT {
            return Err("unsupported native port command".into());
        }
        let (key, width, height, file) = parse_attachment(reader.string()?)?;
        if reader.u32()? != 1 {
            return Err("a raster update needs one bounded byte attachment".into());
        }
        let size = reader.u32()? as usize;
        if size > MAX_PIXELS * 4 {
            return Err("raster attachment exceeds the pixel limit".into());
        }
        let bytes = reader.raw(size)?;
        let source = match (reader.u32()?, file, width, height, size) {
            (0, false, 0, 0, 0) => Source::Ready(None),
            (0, false, width, height, size) if byte_count(width, height) == Some(size) => {
                Source::Raw {
                    width,
                    height,
                    rgba: bytes.to_vec(),
                }
            }
            (1, true, 0, 0, 0) => {
                let decoded =
                    crate::effects::resolve_raster(reader.u64()?, reader.u64()?, reader.u64()?)?;
                Source::Ready(Some(decoded))
            }
            _ => return Err("invalid raster attachment shape".into()),
        };
        Ok(Self { key, source })
    }
}

fn parse_attachment(json: &str) -> Result<(Key, u32, u32, bool)> {
    let parse = || {
        let (key, tail) = json
            .strip_prefix(r#"{"binding":""#)?
            .split_once(r#"","bytes":{"$bytes":0},"files":"#)?;
        let (files, tail) = tail.split_once(r#","height":"#)?;
        let (height, width) = tail.strip_suffix('}')?.split_once(r#","width":"#)?;
        let file = match files {
            "[]" => false,
            r#"[{"$file":0}]"# => true,
            _ => return None,
        };
        Some((
            key.parse().ok()?,
            width.parse().ok()?,
            height.parse().ok()?,
            file,
        ))
    };
    parse().ok_or_else(|| "invalid raster binding metadata".into())
}

#[derive(Default)]
pub(crate) struct Bridge {
    bindings: BTreeSet<Key>,
    assets: BTreeMap<Key, String>,
    pending: BTreeMap<Job, u64>,
    revision: u64,
    worker: Option<Worker>,
}

impl Bridge {
    pub(crate) fn sync(&mut self, tree: &Tree) -> Result<(Vec<Event>, Vec<Update>)> {
        if let Some(worker) = &self.worker {
            worker.retain(tree);
        }
        self.bindings.retain(|key| tree.contains(*key));
        self.assets.retain(|key, _| tree.contains(*key));
        self.pending.retain(|job, _| tree.contains(job.key()));

        let mut events = Vec::new();
        let mut updates = Vec::new();
        for (key, node) in tree.nodes() {
            if let Some((route, listener)) = tree.mounted_event(key, BIND)
                && self.bindings.insert(key)
            {
                events.push(Event::Turn {
                    node: route,
                    handler: listener.handler,
                    payload: key.to_string().into_bytes(),
                });
            }
            if matches!(&node.kind, Kind::Element(tag) if tag == "ui-asset-image") {
                let name = node.attr("asset").unwrap_or("");
                if self.assets.get(&key).is_some_and(|old| old == name) {
                    continue;
                }
                self.assets.insert(key, name.into());
                if name.is_empty() {
                    self.cancel(key);
                } else {
                    let root = std::env::current_exe()
                        .map_err(|error| format!("cannot locate app assets: {error}"))?
                        .with_file_name("assets");
                    self.queue(
                        Job::Source(key),
                        Source::Asset {
                            root,
                            name: name.into(),
                        },
                    )?;
                }
                updates.push((key, None));
            }
        }
        Ok((events, updates))
    }

    pub(crate) fn attach(&mut self, tree: &Tree, attachment: Attachment) -> Result<Option<Update>> {
        let Attachment { key, source } = attachment;
        if !tree.contains(key) || !self.bindings.contains(&key) {
            return Ok(None);
        }
        match source {
            Source::Ready(pixels) => {
                self.cancel(key);
                Ok(Some((key, pixels)))
            }
            _ => {
                self.queue(Job::Source(key), source)?;
                Ok(Some((key, None)))
            }
        }
    }

    fn cancel(&mut self, key: Key) {
        let job = Job::Source(key);
        self.pending.remove(&job);
        if let Some(worker) = &self.worker {
            let mut state = worker.0.0.lock().unwrap();
            state.queued.remove(&job);
            state.completed.remove(&job);
        }
    }

    pub(crate) fn resize(&mut self, key: Key, resize: Resize) -> Result<()> {
        self.queue(Job::Resize(key), Source::Resize(resize))
    }

    fn queue(&mut self, job: Job, source: Source) -> Result<()> {
        if self.worker.is_none() {
            self.worker = Some(Worker::start()?);
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or("raster revision counter exhausted")?;
        let worker = self.worker.as_ref().unwrap();
        worker.queue(job, self.revision, source)?;
        self.pending.insert(job, self.revision);
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Result<(Vec<Update>, Vec<ResizeUpdate>)> {
        let Some(worker) = &self.worker else {
            return Ok((Vec::new(), Vec::new()));
        };
        let ready = std::mem::take(&mut worker.0.0.lock().unwrap().completed);
        let mut sources = Vec::new();
        let mut resized = Vec::new();
        for (job, (revision, result)) in ready {
            if self.pending.get(&job).copied() == Some(revision) {
                self.pending.remove(&job);
                match result? {
                    Prepared::Source(pixels) => sources.push((job.key(), Some(pixels))),
                    Prepared::Resize(source, image) => resized.push(ResizeUpdate {
                        key: job.key(),
                        source,
                        image,
                    }),
                }
            }
        }
        Ok((sources, resized))
    }

    pub(crate) fn ready(&self) -> bool {
        self.pending.is_empty()
    }
}

#[derive(Default)]
struct Mailbox {
    closing: bool,
    queued: BTreeMap<Job, (u64, Source)>,
    completed: BTreeMap<Job, (u64, Result<Prepared>)>,
}

struct Worker(Arc<(Mutex<Mailbox>, Condvar)>);

impl Worker {
    fn start() -> Result<Self> {
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let mailbox = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("roc-image-prepare".into())
            .spawn(move || work(&mailbox))
            .map_err(|error| format!("cannot start the image worker: {error}"))?;
        Ok(Self(shared))
    }

    fn retain(&self, tree: &Tree) {
        let mut mailbox = self.0.0.lock().unwrap();
        mailbox.queued.retain(|job, _| tree.contains(job.key()));
        mailbox.completed.retain(|job, _| tree.contains(job.key()));
    }

    fn queue(&self, job: Job, revision: u64, source: Source) -> Result<()> {
        let (state, ready) = &*self.0;
        let mut state = state.lock().unwrap();
        state.queued.remove(&job);
        state.completed.remove(&job);
        if state.queued.len() >= MAX_QUEUED
            || state
                .queued
                .values()
                .map(|(_, src)| src.queued_bytes())
                .sum::<usize>()
                + source.queued_bytes()
                > MAX_QUEUED_BYTES
        {
            return Err("too much pending image work".into());
        }
        state.queued.insert(job, (revision, source));
        ready.notify_one();
        Ok(())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let (state, ready) = &*self.0;
        let mut state = state.lock().unwrap();
        state.closing = true;
        state.queued.clear();
        state.completed.clear();
        ready.notify_one();
    }
}

type Digest = (u32, u32, [u8; 32]);

fn work((mailbox, ready): &(Mutex<Mailbox>, Condvar)) {
    let mut rasters = BTreeMap::<Digest, Weak<Pixels>>::new();
    let mut assets = BTreeMap::<(PathBuf, String), Weak<Pixels>>::new();
    // The weak source keeps the allocation identity unique while the cache entry lives.
    let mut scaled = BTreeMap::<(usize, u32, u32), (Weak<Pixels>, Weak<Pixels>)>::new();
    loop {
        let (job, (revision, source)) = {
            let mut state = mailbox.lock().unwrap();
            while state.queued.is_empty() && !state.closing {
                state = ready.wait(state).unwrap();
            }
            if state.closing {
                return;
            }
            state.queued.pop_first().unwrap()
        };
        rasters.retain(|_, value| value.strong_count() > 0);
        assets.retain(|_, value| value.strong_count() > 0);
        scaled.retain(|_, (source, image)| source.strong_count() > 0 && image.strong_count() > 0);
        let pixels = match source {
            Source::Raw {
                width,
                height,
                rgba,
            } => {
                let digest = crate::effects::raster_digest(&rgba).map(|hash| (width, height, hash));
                if let Some(pixels) = digest
                    .as_ref()
                    .and_then(|id| rasters.get(id))
                    .and_then(Weak::upgrade)
                {
                    Ok(Prepared::Source(pixels))
                } else {
                    Pixels::from_rgba(width, height, rgba)
                        .map(Arc::new)
                        .inspect(|pixels| {
                            if let Some(digest) = digest {
                                rasters.insert(digest, Arc::downgrade(pixels));
                            }
                        })
                        .map(Prepared::Source)
                }
            }
            Source::Asset { root, name } => {
                let key = (root, name);
                if let Some(pixels) = assets.get(&key).and_then(Weak::upgrade) {
                    Ok(Prepared::Source(pixels))
                } else {
                    decode_asset(&key.0, &key.1)
                        .inspect(|pixels| {
                            assets.insert(key, Arc::downgrade(pixels));
                        })
                        .map(Prepared::Source)
                }
            }
            Source::Resize(Resize {
                source,
                size: (width, height),
            }) => {
                let id = (Arc::as_ptr(&source) as usize, width, height);
                let image = if let Some(pixels) =
                    scaled.get(&id).and_then(|(_, image)| image.upgrade())
                {
                    Ok(pixels)
                } else {
                    source
                        .resize(width, height)
                        .map(Arc::new)
                        .inspect(|pixels| {
                            scaled.insert(id, (Arc::downgrade(&source), Arc::downgrade(pixels)));
                        })
                };
                image.map(|image| Prepared::Resize(source, image))
            }
            Source::Ready(_) => unreachable!("ready image attached on the window thread"),
        };
        let mut state = mailbox.lock().unwrap();
        if !state.closing {
            state.completed.insert(job, (revision, pixels));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_uses_a_mount_id_and_typed_blobs_or_file() {
        assert_eq!(
            parse_attachment(
                r#"{"binding":"18","bytes":{"$bytes":0},"files":[],"height":2,"width":4}"#
            )
            .unwrap(),
            (18, 4, 2, false)
        );
        assert_eq!(
            parse_attachment(
                r#"{"binding":"29","bytes":{"$bytes":0},"files":[{"$file":0}],"height":0,"width":0}"#
            )
            .unwrap(),
            (29, 0, 0, true)
        );
        assert!(
            parse_attachment(
                r#"{"binding":"1","bytes":{"$bytes":0},"files":[{"$file":12}],"height":1,"width":1}"#
            )
            .is_err()
        );
    }
}

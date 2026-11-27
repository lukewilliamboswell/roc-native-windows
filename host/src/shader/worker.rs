use super::d3d::{Gpu, Program, ViewGpu};
use super::{Display, Snapshot, Source};
use crate::Result;
use crate::raster::{Pixels, decode_asset, resolve_asset};
use crate::shader_bytecode::{MAX_BYTES, Reflection, parameters};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

type Surfaces<T> = BTreeMap<u64, T>;
type Images = [Option<Result<Arc<Pixels>>>; 2];
type Frame = (u64, Display);

const FRAME_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Default)]
struct Mailbox {
    live: Surfaces<u64>,
    wanted: Surfaces<Snapshot>,
    decode: Surfaces<Decode>,
    decoded: Surfaces<Decoded>,
    frames: Surfaces<Frame>,
    ready: Surfaces<u64>,
    retired: bool,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<Mailbox>,
    render: Condvar,
    decode: Condvar,
}

pub(super) struct Worker {
    shared: Arc<Shared>,
    render: Option<JoinHandle<()>>,
    decode: Option<JoinHandle<()>>,
}

impl Worker {
    pub(super) fn start() -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let mut worker = Self {
            shared: shared.clone(),
            render: None,
            decode: None,
        };
        let lane = shared.clone();
        worker.decode = Some(
            thread::Builder::new()
                .name("roc-windows-shader-assets".into())
                .spawn(move || decode_loop(&lane))
                .map_err(|err| format!("start shader asset worker: {err}"))?,
        );
        worker.render = Some(
            thread::Builder::new()
                .name("roc-windows-d3d11".into())
                .spawn(move || render_loop(&shared))
                .map_err(|err| format!("start Direct3D worker: {err}"))?,
        );
        Ok(worker)
    }

    pub(super) fn submit(&self, id: u64, snapshot: Snapshot) {
        let mut state = self.shared.state.lock().unwrap();
        state.live.insert(id, snapshot.revision);
        state.wanted.insert(id, snapshot);
        state.frames.remove(&id);
        self.shared.render.notify_one();
    }

    pub(super) fn retire(&self, id: u64) {
        let mut state = self.shared.state.lock().unwrap();
        if state.live.remove(&id).is_some() {
            state.wanted.remove(&id);
            state.decode.remove(&id);
            state.decoded.remove(&id);
            state.ready.remove(&id);
            state.frames.remove(&id);
            state.retired = true;
            self.shared.render.notify_one();
        }
    }

    pub(super) fn frame(&self, id: u64) -> Option<Frame> {
        self.shared.state.lock().unwrap().frames.remove(&id)
    }

    pub(super) fn idle(&self) -> bool {
        let state = self.shared.state.lock().unwrap();
        state.frames.is_empty()
            && state
                .live
                .iter()
                .all(|(id, revision)| state.ready.get(id) == Some(revision))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().stop = true;
        self.shared.render.notify_one();
        self.shared.decode.notify_one();
        // Neither worker owns HWNDs or needs UI message dispatch.
        for lane in [self.render.take(), self.decode.take()]
            .into_iter()
            .flatten()
        {
            let _ = lane.join();
        }
    }
}

struct Decode {
    assets: PathBuf,
    sources: [Source; 2],
    reuse: Images,
}

struct Decoded {
    sources: [Source; 2],
    images: Images,
}

fn decode_loop(shared: &Shared) {
    loop {
        let (id, job) = {
            let mut state = shared.state.lock().unwrap();
            loop {
                if state.stop {
                    return;
                }
                if let Some(item) = state.decode.pop_first() {
                    break item;
                }
                state = shared.decode.wait(state).unwrap();
            }
        };
        let mut images = job.reuse;
        for (index, target) in images.iter_mut().enumerate() {
            if target.is_none() {
                *target = match &job.sources[index] {
                    Source::NoTexture | Source::Waiting => None,
                    Source::Asset(name) => Some(decode_asset(&job.assets, name)),
                    Source::Raster(data) => Some(Ok(data.clone())),
                };
            }
        }
        let mut state = shared.state.lock().unwrap();
        if !state.stop && state.live.contains_key(&id) {
            state.decoded.insert(
                id,
                Decoded {
                    sources: job.sources,
                    images,
                },
            );
            shared.render.notify_one();
        }
    }
}

struct View {
    wanted: Snapshot,
    preparing: Option<[Source; 2]>,
    images: Option<Decoded>,
    program: Result<Arc<Program>>,
    gpu: ViewGpu,
}

impl View {
    fn new(wanted: Snapshot) -> Self {
        let program = load_program(&wanted).map(Arc::new);
        Self {
            wanted,
            preparing: None,
            images: None,
            program,
            gpu: ViewGpu::default(),
        }
    }

    fn update(&mut self, wanted: Snapshot) {
        if self.wanted.shader != wanted.shader || self.wanted.assets != wanted.assets {
            self.program = load_program(&wanted).map(Arc::new);
        }
        self.wanted = wanted;
    }

    fn needs_images(&self) -> bool {
        self.images
            .as_ref()
            .is_none_or(|images| images.sources != self.wanted.sources)
    }

    fn prepare(&mut self, id: u64, shared: &Shared) {
        if self.preparing.as_ref() == Some(&self.wanted.sources) {
            return;
        }
        self.preparing = Some(self.wanted.sources.clone());
        let reuse = std::array::from_fn(|at| {
            let last = self.images.as_ref()?;
            (last.sources[at] == self.wanted.sources[at])
                .then(|| last.images[at].clone())
                .flatten()
        });
        let mut state = shared.state.lock().unwrap();
        if state.live.contains_key(&id) {
            state.decode.insert(
                id,
                Decode {
                    assets: self.wanted.assets.clone(),
                    sources: self.wanted.sources.clone(),
                    reuse,
                },
            );
            shared.decode.notify_one();
        }
    }

    fn display(&mut self, gpu: &Result<Gpu>) -> Display {
        let images = self.images.as_ref().unwrap();
        let result = (|| {
            let program = self.program.as_ref().map_err(Clone::clone)?;
            let gpu = gpu.as_ref().map_err(Clone::clone)?;
            let parameters = parameters(&self.wanted.parameters)?;
            let mut textures = [None, None];
            for (index, needed) in program.reflection.textures.iter().copied().enumerate() {
                if needed {
                    textures[index] = images.images[index].clone().transpose()?;
                }
            }
            gpu.draw(
                &mut self.gpu,
                program,
                &textures,
                &parameters,
                self.wanted.sampling,
                self.wanted.size,
            )
        })();
        match result {
            Ok(frame) => Display::Rendered(frame),
            Err(error) => {
                let original = images.images[0]
                    .as_ref()
                    .and_then(|image| image.as_ref().ok())
                    .cloned();
                Display::Failed(error, original)
            }
        }
    }
}

fn take_batch(shared: &Shared) -> Option<(Surfaces<Snapshot>, Surfaces<Decoded>)> {
    let mut state = shared.state.lock().unwrap();
    while !state.stop && state.wanted.is_empty() && state.decoded.is_empty() && !state.retired {
        state = shared.render.wait(state).unwrap();
    }
    if state.stop {
        return None;
    }
    state.retired = false;
    Some((
        std::mem::take(&mut state.wanted),
        std::mem::take(&mut state.decoded),
    ))
}

fn render_loop(shared: &Shared) {
    let mut views = Surfaces::<View>::new();
    let mut gpu = None;
    let mut deadline = Instant::now();
    while let Some((mut wanted, mut decoded)) = take_batch(shared) {
        // Coalesce new input received during the normal 60 Hz frame interval.
        if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        let live = {
            let mut state = shared.state.lock().unwrap();
            if state.stop {
                return;
            }
            wanted.append(&mut state.wanted);
            decoded.append(&mut state.decoded);
            state.retired = false;
            state.live.keys().copied().collect::<BTreeSet<_>>()
        };
        views.retain(|id, _| live.contains(id));
        let mut dirty = BTreeSet::new();
        for (id, snapshot) in wanted {
            if !live.contains(&id) {
                continue;
            }
            dirty.insert(id);
            if let Some(view) = views.get_mut(&id) {
                view.update(snapshot);
            } else {
                views.insert(id, View::new(snapshot));
            }
        }
        for (id, data) in decoded {
            if let Some(view) = views.get_mut(&id)
                && view.wanted.sources == data.sources
            {
                view.images = Some(data);
                view.preparing = None;
                dirty.insert(id);
            }
        }
        let mut lost = false;
        for id in dirty {
            let view = views.get_mut(&id).unwrap();
            if view.wanted.sources.contains(&Source::Waiting) {
                continue;
            }
            if view.needs_images() {
                view.prepare(id, shared);
                continue;
            }
            if lost {
                continue;
            }
            let device = gpu.get_or_insert_with(Gpu::new);
            let frame = view.display(device);
            if matches!(frame, Display::Failed(..)) && device.as_ref().is_ok_and(Gpu::removed) {
                lost = true;
                continue;
            }
            publish(shared, id, view.wanted.revision, frame);
        }
        if lost {
            // GPU objects are device-specific. Decoded inputs and bytecode can
            // be reused without scheduling another asset read.
            for view in views.values_mut() {
                view.gpu = ViewGpu::default();
            }
            let device = gpu.insert(Gpu::new());
            for (&id, view) in &mut views {
                if !view.needs_images() {
                    publish(shared, id, view.wanted.revision, view.display(device));
                }
            }
        }
        deadline = Instant::now() + FRAME_INTERVAL;
    }
}

fn publish(shared: &Shared, id: u64, revision: u64, frame: Display) {
    let mut state = shared.state.lock().unwrap();
    if state.live.get(&id) == Some(&revision) {
        state.frames.insert(id, (revision, frame));
        state.ready.insert(id, revision);
    }
}

fn load_program(snapshot: &Snapshot) -> Result<Program> {
    let name = crate::asset_catalog::resolve(&snapshot.shader)?;
    if !Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("cso"))
    {
        return Err(format!(
            "expected a .cso shader asset: {:?}",
            snapshot.shader
        ));
    }
    let path = resolve_asset(&snapshot.assets, name)?;
    let mut bytes = Vec::new();
    File::open(&path)
        .and_then(|file| file.take(MAX_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|err| format!("could not read shader {:?}: {err}", snapshot.shader))?;
    let reflection =
        Reflection::read(&bytes).map_err(|err| format!("shader {:?}: {err}", snapshot.shader))?;
    Ok(Program { bytes, reflection })
}

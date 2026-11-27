## A native Windows platform driven by roc-signals. The Rust host applies
## the incremental command stream to Win32 controls and sends native events
## back to Roc. Each UI call runs on the window thread, and workers use
## separate Roc instances with platform-provided file effects.
##
## Engine, Layout, Wire, and Worker re-export modules from the released
## engine package.
platform ""
	requires {
		[Store : st] for main : Engine.Program(st)
	}
	exposes [Clipboard, Engine, Files, Layout, Ui, Wire, Worker]
	packages { engine: "https://github.com/lukewilliamboswell/roc-signals/releases/download/0.8.0-rc2/engine-GKiw1oqhZ1Q3sWgo1mSinHwACzFUCU8cCZPRY7SXYSgh.tar.zst" }
	provides {
		"roc_engine_ask": ask_for_host!,
		"roc_engine_drop": drop_for_host,
		"roc_engine_event": event_for_host,
		"roc_engine_files": files_for_host,
		"roc_engine_forget": forget_for_host,
		"roc_engine_init": init_for_host,
		"roc_engine_keys": keys_for_host,
		"roc_engine_prepare": prepare_for_host!,
		"roc_engine_version": version_for_host,
		"roc_engine_work": work_for_host!,
	}
	hosted {
		"roc_task_append": Hosted.append!,
		"roc_task_append_file": Hosted.append_file!,
		"roc_task_finish": Hosted.finish!,
		"roc_task_mint": Hosted.mint!,
		"roc_task_open": Hosted.open!,
		"roc_task_read": Hosted.read!,
		"roc_task_size": Hosted.size!,
	}
	targets: {
		inputs_dir: "targets/",
		x64mingw: { inputs: ["crt2.obj", "libhost.a", "windows.res", app, "runtime.lib"] },
		arm64mingw: { inputs: ["crt2.obj", "libhost.a", "windows.res", app, "runtime.lib"] },
	}

import engine.EngineHost
import Clipboard
import Effects
import Engine
import Files
import Hosted
import Layout
import Ui
import Wire
import Worker

## One host call's result: the state (owned by the host until the next call),
## this turn's command bytes and the instance's work record (both owned
## by the host and passed back into the next call for reuse).
Step : { cmds : List(U8), state : Handle, work : List(U8) }

## The host keeps the boxed engine state between calls. It passes the box
## back to Roc without reading its contents.
Handle : Box(EngineHost.App(Store))

## The wire protocol version the engine speaks (design.md, Appendix B).
version_for_host : () -> U32
version_for_host = || (EngineHost.driver(main.with_ports(Files.folder_ports()))).version

## The program's declaration sections (2, 5, 6, 10, 11 and 14), which tell
## the host what to read before mount: storage keys, media queries, the zone
## span and the startup profile. roc-signals appends section 12 (protocol
## facts, a CBOR item) here with `ProtocolFacts.append_declaration`; this
## host binds no roles from it and would need the cbor package to build it,
## so the declarations cross as the engine writes them.
keys_for_host : () -> List(U8)
keys_for_host = || ((EngineHost.driver(main.with_ports(Files.folder_ports()))).keys)()

## Mount with the boot bytes the host read for those declarations.
init_for_host : List(U8) -> Step
init_for_host = |boot| {
	(state, cmds, work) = ((EngineHost.driver(main.with_ports(Files.folder_ports()))).init)(boot)
	{ cmds, state: Box.box(state), work }
}

## Route an event (node id, handler number, payload bytes) through one
## turn, reusing the previous call's command and work buffers.
event_for_host : Handle, List(U8), List(U8), U32, U32, List(U8) -> Step
event_for_host = |boxed, buf, wbuf, node, handler, payload| {
	(next, cmds, work) = ((EngineHost.driver(main.with_ports(Files.folder_ports()))).event)(Box.unbox(boxed), buf, wbuf, node, handler, payload)
	{ cmds, state: Box.box(next), work }
}

## Copied from roc-signals `platform/main.roc`: a files-kind event, the file
## record and the handles the host minted for its files, which move into the
## engine with the call.
files_for_host : Handle, List(U8), List(U8), U32, U32, List(U8), List(Box(U64)) -> Step
files_for_host = |boxed, buf, wbuf, node, handler, record, blobs| {
	(next, cmds, work) = ((EngineHost.driver(main.with_ports(Files.folder_ports()))).files)(Box.unbox(boxed), buf, wbuf, node, handler, record, blobs)
	{ cmds, state: Box.box(next), work }
}

## Copied from roc-signals `platform/main.roc`: a worker instance's job
## (design.md, "Workers"): run the task named in the payload
## (`name:str input:rest`) with the handles the host minted for the job's
## files, which move into Roc with the call, and return its answer,
## `n:u32 (token:u64)* output:rest`. The engine gets the platform's hosted
## effects with the call (`Effects.job`), and the job's files and builders
## carry them. A worker instance never mounts.
work_for_host! : List(U8), List(Box(U64)) => List(U8)
work_for_host! = |payload, handles| EngineHost.work!(main, Effects.job, payload, handles)

## Copied from roc-signals `platform/main.roc`: a worker instance's
## preparation (design.md, "Prepared state"): run the preparation named in
## `prep:str init:rest` with the handles of its files and return the state
## the instance keeps, which the host owns until a job runs a task on it or
## the instance forgets it.
prepare_for_host! : List(U8), List(Box(U64)) => Box(EngineHost.Kept)
prepare_for_host! = |payload, handles| Box.box(EngineHost.work_prepare!(main, Effects.job, payload, handles))

## Copied from roc-signals `platform/main.roc`: a job of a preparation's task
## on the held state; the state moves in and back out with the job's answer.
ask_for_host! : Box(EngineHost.Kept), List(U8), List(Box(U64)) => { held : Box(EngineHost.Kept), out : List(U8) }
ask_for_host! = |boxed, payload, handles| {
	state = Box.unbox(boxed)
	out = EngineHost.work_ask!(state, payload, handles)
	{ held: Box.box(state), out }
}

## Let go of the held state: the instance evicts it or prepares another.
forget_for_host : Box(EngineHost.Kept) -> {}
forget_for_host = |_boxed| {}

## Teardown: the state and both buffers are released with the call.
drop_for_host : Handle, List(U8), List(U8) -> {}
drop_for_host = |_boxed, _buf, _wbuf| {}

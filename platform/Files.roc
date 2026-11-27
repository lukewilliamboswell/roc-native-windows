import engine.EngineHost
import engine.Port
import Effects
import Engine

## Building a file in a worker task's job (design.md, "Workers"). A job
## reads the files it was sent, and writes to a builder, with methods
## (`file.read!(...)`, `out.append!(...)`, `out.finish!()`): those files
## and builders carry the job's effects. Opening a builder has no file to
## carry them, so it is the one effect a task calls through this module.
##
## Folder capabilities and worker file builders.
Files := [].{

	FolderSelection : [FolderChosen(Folder), FolderRefused(FolderFailure)]

	FolderFailure : [Unavailable]

	## A chosen directory. The display name omits the location.
	Folder :: { file : Engine.File }.{
		name : Folder -> Str
		name = |{ file }| file.name()

		## Request immediate ordinary files, in filename order, up to the limit.
		files : Folder, FolderFilesProps -> Engine.Ask(FolderListing, FolderFailure)
		files = |{ file }, props| Engine.ask_of(ListFolder.{}, { file, limit: props.limit.max(1).min(16) })
	}

	## Defaults to 16 files. The supported range is 1 through 16.
	FolderFilesProps := { limit : U8 ?? 16 }

	FolderListing : {
		files : List(Engine.File),
		completeness : [Complete, Partial({ limit_reached : Bool, skipped_entries : Bool })],
	}

	FolderButtonProps(ctx) := { on_folder : Engine.HandlerOf(ctx, FolderSelection) }

	folder_button : Engine.Signal(ctx, Str), FolderButtonProps(ctx) -> Engine.Elem(ctx)
	folder_button = |title, props| {
		handler = props.on_folder.from_payload_by(
			Engine.const({}),
			|_, { files, refused }| match (files, refused) {
				([file], 0) => FolderChosen(Folder.{ file })
				_ => FolderRefused(Unavailable)
			},
		)
		Engine.element("ui-folder-button", [Engine.on_files(Change, handler)], [Engine.text_s(title)])
	}

	folder_ports : () -> List(Engine.PortRegistration)
	folder_ports = || Port.registrations({ folders: list_port })

	## How a builder starts: the new file's `name`, its media `mime` type
	## (`Auto`: the type of the file it starts from, or none), and what it
	## starts with: nothing (`Empty`), or the bytes of a file the job holds
	## (`From(file)`), by reference, without copying them.
	BuildProps : EngineHost.BuildProps

	## In a task's job: a new builder for a file the job writes. Append to
	## it with `append!` and `append_file!`, then `finish!` it into an
	## immutable file the job can return in its answer. The bytes it holds
	## are private to the job until its answer is delivered: a job that is
	## stopped, times out or fails leaves nothing anyone can see. A builder
	## never outlives the call that opened it (a job, or a preparation's
	## `prepare!`): one kept in a prepared state and used later is a
	## contract error. Keep the file it finishes instead.
	##
	## To append to the output of an earlier job, start from it:
	## `Files.build!({ name, start_from: From(previous) })` gives a new file
	## that is the previous one followed by what this job appends, and the
	## previous file is unchanged whatever happens to this job.
	build! : BuildProps => Engine.Builder
	build! = |props| EngineHost.open_builder!(Effects.job, props)
}

ListFolder := {}.{
	request : ListFolder, { file : Engine.File, limit : U8 } -> Engine.Request
	request = |_, { file, limit }| list_port.request({ folder: file, limit })

	result : ListFolder, Engine.Result -> Try(Files.FolderListing, Files.FolderFailure)
	result = |_, answer| match answer {
		PortAnswer(_) => match list_port.result(answer) {
			Ok({ flags: 0, files }) => Ok({ files, completeness: Complete })
			Ok({ flags, files }) if flags >= 1 and flags <= 3 =>
				Ok({ files, completeness: Partial({ limit_reached: flags != 2, skipped_entries: flags != 1 }) })
			_ => Err(Unavailable)
		}
		_ => Err(Unavailable)
	}
}

list_port : Port.Call({ folder : Engine.File, limit : U8 }, { files : List(Engine.File), flags : U8 })
list_port = Port.call({ name: "roc-native-windows/folder-files", timeout: Ms(10000) })

#!/usr/bin/env roc-stable
app [main!] {
	cli: platform "https://github.com/roc-lang/basic-cli/releases/download/0.28.1/7brgwR8YUqfeGrkdfF1V4GPPN7DnP7ftqub9KcrTEZtr.tar.zst",
	roc: "nightly-2026-11-26-890d6c8",
	weaver: "https://github.com/lukewilliamboswell/weaver/releases/download/0.10.0/7CdYzjBAJmprmnK45oTpmH6BXt76ZASN2Ck5uZQEoWsK.tar.zst",
}

import cli.Duration
import cli.OsStr
import cli.Path
import cli.Stderr
import cli.Stdout
import cli.Tcp
import weaver.Cli
import weaver.Opt
import weaver.Param

parser : Cli.CliParser({ directory : Str, port : U16 })
parser =
	Cli.assert_valid(
		Cli.finish(
			{
				port: Opt.u16({ short: "", long: "port", help: "Loopback port. Use 0 for an available port.", default: Value(8000) }),
				directory: Param.str({ name: "directory", help: "Directory of staged release files.", default: NoDefault }),
			}.Cli,
			{
				name: "serve",
				description: "Serve staged release assets over loopback HTTP.",
				version: "development",
				authors: [],
				text_style: Plain,
			},
		),
	)

main! = |raw_args| {
	args = if (raw_args.first() == Ok(OsStr.from_str("--"))) raw_args.drop_first(1) else raw_args
	match Cli.parse_or_display_message(parser, args, OsStr.to_raw) {
		Ok(options) => serve!(options)
		Err(Help(message)) | Err(Version(message)) => Stdout.line!(message)
		Err(InvalidUsage(message)) => {
			Stderr.line!(message)?
			Err(Exit(2))
		}
	}
}

serve! = |{ directory, port }| {
	root = Path.utf8(directory)
	if !Path.is_dir!(root)? {
		Stderr.line!("Expected a release directory at ${directory}.")?
		return Err(Exit(1))
	}
	listener = Tcp.listen!("127.0.0.1", port, Duration.seconds(5))?
	bound = listener.local_port!()?
	Stdout.line!("Serving ${directory} at http://127.0.0.1:${bound.to_str()}/")?
	loop!(listener, root)
}

loop! = |listener, root| {
	while Bool.True {
		match listener.accept!({ accept_timeout: NoTimeout, io_timeout: Duration.seconds(5) }) {
			Ok(conn) => respond!(conn.with_line_limit(8192), root) ?? {}
			Err(Interrupted) => return listener.close!()
			Err(error) => return Err(error)
		}
	}
}

respond! = |conn, root| {
	line = conn.read_line!()?
	headers!(conn)?
	match request(line) {
		Ok({ head, name: "" }) => reply!(conn, head, "200 OK", "Release preview\n".to_utf8())
		Ok({ head, name }) => {
			path = root.join(name)
			if Path.is_file!(path)? {
				bytes = Path.read_bytes!(path)?
				reply!(conn, head, "200 OK", bytes)
			} else {
				reply!(conn, head, "404 Not Found", "Not found\n".to_utf8())
			}
		}
		Err(Method) => reply!(conn, Bool.False, "405 Method Not Allowed", "Method not allowed\n".to_utf8())
		Err(BadRequest) => reply!(conn, Bool.False, "400 Bad Request", "Bad request\n".to_utf8())
	}
}

headers! = |conn| {
	for _ in (0.U64).until(32) {
		if (conn.read_line!()? == "") return Ok({})
	}
	Err(TooManyHeaders)
}

reply! = |conn, head, status, bytes| {
	conn.write_utf8!("HTTP/1.1 ${status}\r\nConnection: close\r\nContent-Length: ${bytes.len().to_str()}\r\nContent-Type: application/octet-stream\r\n\r\n")?
	if (!head) conn.write_bytes!(bytes)? else {}
	Ok({})
}

request = |line| match line.split_on(" ") {
	[method, target, version] if ["HTTP/1.0", "HTTP/1.1"].contains(version) => {
		if (!["GET", "HEAD"].contains(method)) return Err(Method)
		name = target.drop_prefix("/")
		if target == "/" or (target.starts_with("/") and safe_name(name)) {
			Ok({ head: method == "HEAD", name })
		} else Err(BadRequest)
	}
	_ => Err(BadRequest)
}

safe_name = |name|
	!name.is_empty() and !name.starts_with(".") and name.to_utf8().all(
		|byte| (byte >= 'A' and byte <= 'Z') or (byte >= 'a' and byte <= 'z') or (byte >= '0' and byte <= '9') or ['-', '_', '.'].contains(byte),
	)

expect request("GET /windows-0_1_0-HASH.tar.zst HTTP/1.1") == Ok({ head: Bool.False, name: "windows-0_1_0-HASH.tar.zst" })
expect request("HEAD / HTTP/1.0") == Ok({ head: Bool.True, name: "" })
expect request("POST / HTTP/1.1") == Err(Method)
expect ["GET /../LICENSE HTTP/1.1", "GET /%2e%2e HTTP/1.1", "GET /folder\\file HTTP/1.1"].all(|line| request(line) == Err(BadRequest))

# Logging

`init` installs a `tracing` subscriber writing to two places: the module's
stderr, which the host forwards into its own log, and a file in the instance
directory, which `get_log_path()` names so a consumer can hand the run over
afterwards.

## What is logged

Three targets carry the chat core's account of a run: `libchat` (the
conversation core, MLS groups, inbox), `logos_generic_chat` (the threaded client
and its inbound worker), and `chat_module` itself. The module is one of them
because the other two are nearly silent: between them they raise eleven events,
almost all on paths that are already failing, so a run that merely behaves oddly
would write nothing. This module reports its own lifecycle instead, and logs a
message as a byte count and a conversation id, never as content.

## Choosing a level

`log_level` on the `ChatConfig` record sets those three targets and nothing else
(`error`, `warn`, `info`, `debug` or `trace`, defaulting to `info`), leaving
everything around them at `warn`, because the crates underneath the chat core
have an order of magnitude more `info` sites than it does.

`RUST_LOG`, read from the environment the module process inherits from its host,
outranks the client's choice and replaces the composition outright, so a verbose
run names every target it wants:

```
RUST_LOG=warn,chat_module=debug,libchat=debug,logos_generic_chat=debug
```

The level is read once, at the first `init`, and a later `init` leaves it as it
was.

## Panics

A panic goes into the log file too, with a backtrace. It cannot arrive as a
`tracing` event, since `panic = "abort"` means the process is already on its way
down, so the panic hook writes it directly and captures the backtrace whether or
not `RUST_BACKTRACE` was set.

## The files

The file is `chat_module_<stamp>.log`, moved aside as
`chat_module_<stamp>.NNN.log` when it fills and reopened under the announced
name, with the ten most recent runs kept. That naming is what lets a consumer
group the directory into runs — and lets a second writer keep its own log
alongside without either knowing about the other, since the grouping reads the
stem off the announced path.

## Line format

A line on stderr is `<SEVERITY>: <target>: <message>`, with `WARNING` for a
warning because that is the token a host ranks the line by, and with no timestamp
because the host stamps what it re-emits. A reader downstream gets the level from
the host's own column and the domain from the target. The same line in the file
leads with an ISO-8601 time, because nothing else stamps it and reading it
against another writer's log takes one.

Only the stderr half meets that host classifier, which is why `debug` and `trace`
are worth asking for: a host drops what it ranks below its own level, and the
file is written directly and drops nothing.

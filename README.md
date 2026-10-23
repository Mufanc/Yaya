# Yaya

An Android runtime debugger for developers and AI agents. yaya turns JDWP into explicit CLI commands for exploring a running app, stopping at a specific instruction, inspecting its state, and testing changes directly in the live process.

It complements [apkq](https://github.com/Mufanc/apkq): apkq explains what is in an APK; yaya shows what happens when that code runs. Use original runtime signatures and DEX instruction offsets to connect static analysis with a live debugging session.

## What it can do

- **Explore runtime types and objects.** Find loaded classes, inspect methods and inherited fields, enumerate instances, and read or write selected fields. Resolve class-loader ambiguity with explicit type IDs.
- **Stop at precise code locations.** Set breakpoints using full method signatures and DEX offsets, choosing whether to suspend the event thread, all threads, or neither.
- **Inspect execution state.** List threads, inspect call stacks, and read or write explicitly typed registers in suspended frames.
- **Control execution.** Step into, over or out of methods, then resume a thread or the whole VM. Stepping returns after registration and resume; events report what actually happens next.
- **Observe debugging events.** Drain queued events, wait for the next batch, or collect events for a duration.
- **Access the underlying protocol.** Send raw JDWP requests through the same connection and event queue when a higher-level command is not enough.

## Designed for agent workflows

**A persistent session across separate commands.** Each session has its own daemon and explicit ID, so an agent can issue commands from independent shell invocations without keeping an interactive terminal alive. A session accepts one client at a time and ends on target disconnect or after 30 minutes without an active client.

**Interactive when you want it.** Foreground interactive mode uses the same command execution and event handling as daemon mode, with an in-process channel instead of a Unix socket.

**Structured results, readable help.** Results, events and execution errors use JSONL with explicit command completion records. Help and argument diagnostics remain plain text.

**Acknowledged event delivery.** Events stay queued until acknowledged after output is written and flushed. Reconnection can replay unacknowledged batches with stable sequence numbers. A bounded queue reports dropped packets when it overflows.

**Explicit control over the target.** yaya does not automatically suspend for reads or writes, retry operations, cancel a step because another breakpoint fired, or maintain a shadow copy of the target's running state. Writes are direct; there is no implicit rollback or undo. Detach releases the debugger connection without terminating the app.

## Guidance

The CLI includes detailed help for every command, covering usage, examples, behavior and limitations. The help text is maintained in the [help directory](help/). For guidance on a specific debugging task, ask an LLM and have it consult the relevant command help to plan the steps.

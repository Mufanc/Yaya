use std::io::{IsTerminal, Write};
use std::sync::Arc;

use clap::Parser;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::cli::{Command, CurrentSession, InteractiveArgs, InteractiveCommand};
use crate::commands::{Core, Data};
use crate::jdwp::{Failure, JdwpClient};
use crate::reporter::Reporter;

use super::server::Server;
use super::{client, transport};

pub async fn run(host: String, port: u16) -> anyhow::Result<bool> {
    let mut reporter = Reporter::new();
    let result = async {
        let client = JdwpClient::connect(&host, port).await?;
        let core = Core {
            client,
            session: super::new_id(),
        };

        reporter.set_session(&core.session);
        reporter.result(Data::Attached {
            endpoint: core.client.endpoint.clone(),
        })?;

        Ok::<_, anyhow::Error>(core)
    }
    .await;
    let core = match result {
        Ok(core) => {
            reporter.finish(Ok("completed"))?;
            core
        }
        Err(error) => return reporter.finish(Err(error)),
    };
    let id = core.session.clone();

    match interact(core).await {
        Ok(ok) => Ok(ok),
        Err(error) => {
            let mut reporter = Reporter::new();
            reporter.set_session(&id);
            reporter.finish(Err(error))
        }
    }
}

async fn execute(
    server: Arc<Server>,
    id: &str,
    command: Command<CurrentSession>,
    reporter: &mut Reporter,
) -> anyhow::Result<&'static str> {
    let (mut connection, accepted) = transport::channel();
    let task = tokio::spawn(server.serve(accepted));
    let result = client::execute(&mut connection, id, command, reporter).await;

    drop(connection);

    let _ = task.await;

    result
}

async fn interact(core: Core) -> anyhow::Result<bool> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut disconnected = core.client.disconnected.subscribe();
    let mut pending_line = None;
    let client = core.client.clone();
    let id = core.session.clone();
    let server = Server::new(core);

    loop {
        if client.disconnected.borrow().is_some() {
            return Err(Failure::new("disconnected", "target disconnected").into());
        }

        if std::io::stdin().is_terminal() {
            eprint!("yaya> ");
        }

        let line = if let Some(line) = pending_line.take() {
            Some(line)
        } else {
            tokio::select! {
                line = lines.next_line() => line?,
                _ = tokio::signal::ctrl_c() => continue,
                _ = disconnected.changed() => return Err(Failure::new("disconnected", "target disconnected").into()),
            }
        };
        let Some(line) = line else {
            client.close().await;
            return Ok(true);
        };
        let Some(words) = shlex::split(&line) else {
            eprintln!("error: unclosed quote or escape");
            continue;
        };

        if words.is_empty() {
            continue;
        }

        let args = match InteractiveArgs::try_parse_from(
            std::iter::once("yaya").chain(words.iter().map(String::as_str)),
        ) {
            Ok(args) => args,
            Err(error) => {
                write!(std::io::stderr().lock(), "{error}")?;
                continue;
            }
        };
        let command = match args.command {
            InteractiveCommand::Debug(command) => command,
            InteractiveCommand::Exit => Command::Detach {
                session: CurrentSession {},
            },
        };

        let mut reporter = Reporter::new();

        reporter.set_session(&id);

        if matches!(command, Command::Events { .. }) {
            let mut eof = false;
            let result = {
                let events = execute(server.clone(), &id, command, &mut reporter);

                tokio::pin!(events);

                loop {
                    tokio::select! {
                        result = &mut events => break result,
                        line = lines.next_line(), if pending_line.is_none() => {
                            match line {
                                Ok(Some(line)) => pending_line = Some(line),
                                Ok(None) => {
                                    eof = true;
                                    break Ok("eof");
                                }
                                Err(error) => break Err(error.into()),
                            }
                        }
                    }
                }
            };

            let ok = reporter.finish(result)?;

            if !ok && client.disconnected.borrow().is_some() {
                return Ok(false);
            }

            if eof {
                client.close().await;
                return Ok(true);
            }

            continue;
        }

        let exit = matches!(command, Command::Detach { .. });
        let result = execute(server.clone(), &id, command, &mut reporter).await;

        let ok = reporter.finish(result)?;

        if exit || (!ok && client.disconnected.borrow().is_some()) {
            return Ok(ok);
        }
    }
}

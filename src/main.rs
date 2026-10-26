mod cli;
mod commands;
mod events;
mod jdwp;
mod reporter;
mod session;
mod wire;

use clap::Parser;
use tokio::runtime::Builder;

use crate::cli::{Args, DaemonCommand, EntryCommand};
use crate::reporter::Reporter;
use crate::session::daemon::DaemonStartup;

fn main() {
    let args = Args::parse();

    let result = run(args);

    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(1),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}

fn run(args: Args) -> anyhow::Result<bool> {
    match args.command {
        EntryCommand::Sessions => {
            let mut reporter = Reporter::new();
            let result = session::sessions().and_then(|data| {
                reporter.result(data)?;
                Ok("completed")
            });

            reporter.finish(result)
        }
        EntryCommand::Attach {
            host,
            port,
            interactive,
        } => {
            if interactive {
                block_on(session::interactive::run(host, port))
            } else {
                match DaemonStartup::fork() {
                    Ok(startup) => block_on(startup.run(host, port)),
                    Err(error) => Reporter::new().finish(Err(error)),
                }
            }
        }
        EntryCommand::SessionCommand(command) => {
            let mut reporter = Reporter::new();
            let result = block_on(async {
                let id = session::selected(command.session().map(str::to_owned))?;

                reporter.set_session(&id);

                let mut connection = session::client::connect(&id).await?;

                session::client::execute(&mut connection, &id, command, &mut reporter).await
            });

            reporter.finish(result)
        }
        EntryCommand::Daemon {
            session,
            command: DaemonCommand::Stop,
        } => {
            let mut reporter = Reporter::new();
            let result = block_on(async {
                let id = session::selected(session.session)?;

                reporter.set_session(&id);

                let mut connection = session::client::connect(&id).await?;
                let data = session::client::stop(&mut connection, &id).await?;

                reporter.result(data)?;
                Ok("completed")
            });

            reporter.finish(result)
        }
    }
}

fn block_on<T>(task: impl Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    // TODO: Verify whether synchronous I/O or other blocking work delays JDWP reception and event handling on this single-thread runtime.
    let runtime = Builder::new_current_thread().enable_all().build()?;
    let result = runtime.block_on(task);

    // Interactive stdin may leave a blocking read pending at shutdown.
    runtime.shutdown_background();

    result
}

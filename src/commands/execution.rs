use anyhow::{Context, bail, ensure};

use crate::cli::Break;
use crate::jdwp::event_request::{EventKind, Modifier, StepDepth, StepSize, SuspendPolicy};
use crate::jdwp::location::Location;
use crate::jdwp::{Failure, event_request, thread_reference, virtual_machine};
use crate::wire;

use super::Core;
use super::data::{Data, ThreadInfo};

impl Core {
    pub(super) async fn breakpoint(&self, command: Break) -> anyhow::Result<Data> {
        match command {
            Break::Add {
                signature,
                pc,
                type_id,
                suspend,
            } => {
                let (class, method) = signature
                    .split_once("->")
                    .context("method signature requires Class;->name(args)return")?;
                let (name, tail) = method
                    .split_once('(')
                    .context("method descriptor missing")?;
                let descriptor = format!("({tail}");
                let selected = self.resolve(class, type_id).await?;
                let methods = self.methods(&selected).await?;
                let method = methods
                    .iter()
                    .find(|method| method.name == name && method.signature == descriptor)
                    .context("method not found")?;
                let suspend_policy = match suspend.as_str() {
                    "none" => SuspendPolicy::None,
                    "thread" => SuspendPolicy::EventThread,
                    "all" => SuspendPolicy::All,
                    _ => bail!("invalid suspend policy"),
                };
                let request_id = self
                    .client
                    .request(event_request::Set {
                        kind: EventKind::Breakpoint,
                        suspend_policy,
                        modifiers: vec![Modifier::LocationOnly(Location {
                            type_tag: selected.type_tag,
                            type_id: super::parse_id(&selected.type_id)?,
                            method_id: super::parse_id(&method.id)?,
                            code_index: pc,
                        })],
                    })
                    .await?;

                Ok(Data::EventRequest {
                    event_request_id: request_id,
                    operation: "break_add".into(),
                })
            }

            Break::Remove { id } => {
                self.client
                    .request(event_request::Clear {
                        kind: EventKind::Breakpoint,
                        request_id: id,
                    })
                    .await?;

                Ok(super::confirmed("break_remove"))
            }

            Break::Clear => {
                self.client
                    .request(event_request::ClearAllBreakpoints)
                    .await?;

                Ok(super::confirmed("clear_all_breakpoints"))
            }
        }
    }

    pub(super) async fn threads(&self) -> anyhow::Result<Data> {
        let thread_ids = self.client.request(virtual_machine::AllThreads).await?;
        let mut threads = Vec::new();

        for thread_id in thread_ids {
            let thread = thread_id.0;
            let name = self
                .client
                .request(thread_reference::Name { thread })
                .await?;
            let state = self
                .client
                .request(thread_reference::Status { thread })
                .await?;

            threads.push(ThreadInfo {
                thread: wire::id(thread),
                name,
                status: state.status,
                suspend_status: state.suspend_status,
            });
        }

        Ok(Data::Threads { threads })
    }

    pub(super) async fn step(
        &self,
        thread: u64,
        into: bool,
        over: bool,
        out: bool,
    ) -> anyhow::Result<Data> {
        ensure!(
            [into, over, out].into_iter().filter(|value| *value).count() == 1,
            "choose one step depth"
        );

        let depth = if into {
            StepDepth::Into
        } else if over {
            StepDepth::Over
        } else {
            StepDepth::Out
        };
        let request_id = self
            .client
            .request(event_request::Set {
                kind: EventKind::SingleStep,
                suspend_policy: SuspendPolicy::EventThread,
                modifiers: vec![
                    Modifier::Step {
                        thread,
                        size: StepSize::Min,
                        depth,
                    },
                    Modifier::Count(1),
                ],
            })
            .await?;

        if let Err(error) = self
            .client
            .request(thread_reference::Resume { thread })
            .await
        {
            let mut failure = Failure::from_error(&error);

            failure.event_request_id = Some(request_id);
            failure.message = format!(
                "step request {request_id} created; resume failed or unknown; request was not rolled back: {}",
                failure.message
            );

            return Err(failure.into());
        }

        Ok(Data::EventRequest {
            event_request_id: request_id,
            operation: "step_started".into(),
        })
    }

    pub(super) async fn resume(&self, thread: Option<u64>, all: bool) -> anyhow::Result<Data> {
        ensure!(thread.is_some() != all, "choose thread or all");

        if let Some(thread) = thread {
            self.client
                .request(thread_reference::Resume { thread })
                .await?;
        } else {
            self.client.request(virtual_machine::Resume).await?;
        }

        Ok(super::confirmed("resume_once"))
    }
}

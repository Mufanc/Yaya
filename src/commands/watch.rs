use anyhow::{Context, ensure};

use crate::cli::{ClassPrepare, Exception, Field, FieldWatch, WatchOptions};
use crate::jdwp::event_request::{Clear, EventKind, Modifier, Set, SuspendPolicy};
use crate::jdwp::reference_type::Fields;

use super::{Core, Data};

impl Core {
    async fn watch(
        &self,
        kind: EventKind,
        modifier: Modifier,
        options: WatchOptions,
        operation: &str,
    ) -> anyhow::Result<Data> {
        let mut modifiers = vec![modifier];

        if let Some(thread) = options.thread {
            modifiers.push(Modifier::ThreadOnly(thread));
        }

        // Count only events that passed every other filter.
        if let Some(count) = options.count {
            modifiers.push(Modifier::Count(count));
        }

        let request_id = self
            .client
            .request(Set {
                kind,
                suspend_policy: SuspendPolicy::parse(&options.suspend)?,
                modifiers,
            })
            .await?;

        Ok(Data::EventRequest {
            event_request_id: request_id,
            operation: operation.into(),
        })
    }

    async fn unwatch(&self, kind: EventKind, id: u32, operation: &str) -> anyhow::Result<Data> {
        self.client
            .request(Clear {
                kind,
                request_id: id,
            })
            .await?;

        Ok(super::confirmed(operation))
    }

    pub(super) async fn exception(&self, command: Exception) -> anyhow::Result<Data> {
        match command {
            Exception::Watch {
                class,
                all,
                type_id,
                with_caught,
                caught_only,
                options,
            } => {
                ensure!(class.is_some() != all, "choose an exception class or --all");
                ensure!(
                    !with_caught || !caught_only,
                    "--with-caught and --caught-only conflict"
                );
                ensure!(
                    class.is_some() || type_id.is_none(),
                    "--type-id requires an exception class"
                );

                let exception = if let Some(class) = class {
                    super::parse_id(&self.resolve(&class, type_id).await?.type_id)?
                } else {
                    0
                };

                self.watch(
                    EventKind::Exception,
                    Modifier::ExceptionOnly {
                        exception,
                        caught: with_caught || caught_only,
                        uncaught: !caught_only,
                    },
                    options,
                    "exception_watch",
                )
                .await
            }
            Exception::Unwatch { id } => {
                self.unwatch(EventKind::Exception, id, "exception_unwatch")
                    .await
            }
        }
    }

    pub(super) async fn class_prepare(&self, command: ClassPrepare) -> anyhow::Result<Data> {
        match command {
            ClassPrepare::Watch { pattern, options } => {
                self.watch(
                    EventKind::ClassPrepare,
                    Modifier::ClassMatch(pattern),
                    options,
                    "class_prepare_watch",
                )
                .await
            }
            ClassPrepare::Unwatch { id } => {
                self.unwatch(EventKind::ClassPrepare, id, "class_prepare_unwatch")
                    .await
            }
        }
    }

    pub(super) async fn field_watch(&self, command: Field) -> anyhow::Result<Data> {
        let (kind, command, watch_operation, unwatch_operation) = match command {
            Field::Access { command } => (
                EventKind::FieldAccess,
                command,
                "field_access_watch",
                "field_access_unwatch",
            ),
            Field::Modification { command } => (
                EventKind::FieldModification,
                command,
                "field_modification_watch",
                "field_modification_unwatch",
            ),
        };

        match command {
            FieldWatch::Watch {
                class,
                field,
                type_id,
                options,
            } => {
                let selected = self.resolve(&class, type_id).await?;
                let declaring = super::parse_id(&selected.type_id)?;
                let fields = self.client.request(Fields { type_id: declaring }).await?;
                let mut matches = fields.0.iter().filter(|member| member.name == field);
                let member = matches.next().context("declared field not found")?;

                ensure!(
                    matches.next().is_none(),
                    "ambiguous declared field: {field}"
                );

                self.watch(
                    kind,
                    Modifier::FieldOnly {
                        declaring,
                        field: member.id,
                    },
                    options,
                    watch_operation,
                )
                .await
            }
            FieldWatch::Unwatch { id } => self.unwatch(kind, id, unwatch_operation).await,
        }
    }
}

use std::io::{self, Stdout, Write};

use anyhow::anyhow;

use serde::{Deserialize, Serialize};

use crate::commands::Data;
use crate::events::{Delivery, EventBatch, Payload};
use crate::jdwp::Failure;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    Result {
        session: Option<String>,
        data: Data,
    },
    EventBatch {
        session: String,
        sequence: u64,
        batch: EventBatch,
    },
    Notice {
        session: String,
        sequence: u64,
        dropped_packets: u64,
    },
    Error {
        session: Option<String>,
        error: Failure,
    },
    End {
        ok: bool,
        reason: String,
    },
}

impl Record {
    pub fn event(session: String, delivery: Delivery) -> Self {
        match delivery.payload {
            Payload::Events { batch } => Record::EventBatch {
                session,
                sequence: delivery.sequence,
                batch,
            },
            Payload::Dropped { packets } => Record::Notice {
                session,
                sequence: delivery.sequence,
                dropped_packets: packets,
            },
        }
    }
}

pub struct Reporter<W: Write = Stdout> {
    session: Option<String>,
    writer: W,
    finished: bool,
}

impl Reporter {
    pub fn new() -> Self {
        Self {
            session: None,
            writer: io::stdout(),
            finished: false,
        }
    }
}

impl<W: Write> Reporter<W> {
    pub fn set_session(&mut self, session: &str) {
        self.session = Some(session.to_owned());
    }

    fn emit(&mut self, record: Record) -> anyhow::Result<()> {
        let mut line = serde_json::to_vec(&record)?;
        line.push(b'\n');
        self.writer.write_all(&line)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn result(&mut self, data: Data) -> anyhow::Result<()> {
        self.emit(Record::Result {
            session: self.session.clone(),
            data,
        })
    }

    pub fn event(&mut self, delivery: Delivery) -> anyhow::Result<()> {
        let session = self
            .session
            .clone()
            .ok_or_else(|| anyhow!("event requires a session"))?;

        self.emit(Record::event(session, delivery))
    }

    pub fn finish(mut self, result: anyhow::Result<&str>) -> anyhow::Result<bool> {
        self.finished = true;

        let (ok, reason) = match result {
            Ok(reason) => (true, reason),
            Err(error) => {
                self.emit(Record::Error {
                    session: self.session.clone(),
                    error: Failure::from_error(&error),
                })?;

                (false, "error")
            }
        };

        self.emit(Record::End {
            ok,
            reason: reason.into(),
        })?;

        Ok(ok)
    }
}

impl<W: Write> Drop for Reporter<W> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert!(self.finished, "Reporter dropped without calling finish");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Error;

    use serde_json::Value;

    use super::*;

    #[test]
    fn finish_reports_success_or_error_once() {
        for failed in [false, true] {
            let mut buffer = Vec::new();
            let reporter = Reporter {
                session: Some("s_abc123".into()),
                writer: &mut buffer,
                finished: false,
            };
            let result = if failed {
                Err(anyhow!("test failure"))
            } else {
                Ok("timeout")
            };
            assert_eq!(reporter.finish(result).unwrap(), !failed);
            let records: Vec<Value> = String::from_utf8(buffer)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(records.len(), if failed { 2 } else { 1 });
            let end = records.last().unwrap();
            assert_eq!(end["kind"], "end");
            assert_eq!(end["ok"], !failed);
            assert_eq!(end["reason"], if failed { "error" } else { "timeout" });
            if failed {
                assert_eq!(records[0]["kind"], "error");
                assert_eq!(records[0]["session"], "s_abc123");
            }
        }
    }

    #[test]
    fn flush_failure_is_returned() {
        struct FailFlush;
        impl Write for FailFlush {
            fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
                Ok(buffer.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(Error::other("flush failed"))
            }
        }
        let mut reporter = Reporter {
            session: Some("s_abc123".into()),
            writer: FailFlush,
            finished: false,
        };
        assert!(
            reporter
                .event(Delivery {
                    sequence: 1,
                    payload: Payload::Dropped { packets: 3 }
                })
                .is_err()
        );
        assert!(reporter.finish(Ok("completed")).is_err());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    #[should_panic(expected = "Reporter dropped without calling finish")]
    fn unfinished_reporter_panics() {
        let _reporter = Reporter {
            session: None,
            writer: Vec::new(),
            finished: false,
        };
    }

    #[test]
    fn unwinding_does_not_panic_again() {
        let panic = std::panic::catch_unwind(|| {
            let _reporter = Reporter {
                session: None,
                writer: Vec::new(),
                finished: false,
            };
            panic!("original panic");
        })
        .unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"original panic"));
    }
}

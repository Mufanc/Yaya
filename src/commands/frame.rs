use anyhow::{Context, Error, ensure};

use crate::cli::Frame;
use crate::jdwp::stack_frame::{GetValues, SetValues, Slot, ThisObject};
use crate::jdwp::thread_reference::Frames;
use crate::jdwp::value::Value;
use crate::wire;

use super::Core;
use super::data::{Data, FrameInfo};

impl Core {
    pub(super) async fn frame(&self, command: Frame) -> anyhow::Result<Data> {
        match command {
            Frame::This { frame, thread } => {
                let object = self.client.request(ThisObject { thread, frame }).await?;
                Ok(Data::ThisObject {
                    thread: wire::id(thread),
                    frame: wire::id(frame),
                    object: object.into(),
                })
            }
            Frame::Get {
                frame,
                thread,
                registers,
            } => {
                let mut names = Vec::new();
                let mut slots = Vec::new();

                for register in registers {
                    let (name, kind) = register
                        .split_once(':')
                        .context("register needs explicit type, e.g. v0:int")?;

                    slots.push(Slot {
                        index: slot(name)?,
                        tag: wire::value_tag(kind).map_err(Error::msg)?,
                    });
                    names.push(name.to_owned());
                }

                let values = self
                    .client
                    .request(GetValues {
                        thread,
                        frame,
                        slots,
                    })
                    .await?;

                let registers = names
                    .into_iter()
                    .zip(values.into_iter().map(Into::into))
                    .collect();

                Ok(Data::Registers { registers })
            }

            Frame::Set {
                frame,
                thread,
                register,
                tag,
                value,
            } => {
                let request = SetValues {
                    thread,
                    frame,
                    slots: vec![(slot(&register)?, Value::parse(tag, &value)?)],
                };

                self.client.request(request).await?;

                Ok(super::confirmed("frame_set"))
            }
        }
    }

    pub(super) async fn stack(&self, thread: u64) -> anyhow::Result<Data> {
        let frames = self
            .client
            .request(Frames {
                thread,
                start: 0,
                length: -1,
            })
            .await?;
        let frames = frames
            .into_iter()
            .map(|frame| FrameInfo {
                frame: wire::id(frame.id),
                thread: wire::id(thread),
                location: frame.location.into(),
            })
            .collect();

        Ok(Data::Frames { frames })
    }
}

fn slot(value: &str) -> anyhow::Result<u32> {
    // shortcut: vN targets modern ART's direct slots; legacy ART requires explicit slotN.
    let digits = value
        .strip_prefix("slot")
        .or_else(|| value.strip_prefix('v'))
        .context("use vN or slotN")?;

    ensure!(
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()),
        "slot requires decimal digits"
    );

    let slot = digits.parse::<u32>()?;

    ensure!(slot <= i32::MAX as u32, "slot exceeds signed JDWP integer");

    Ok(slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modern_art_registers_are_direct_slots() {
        assert_eq!(slot("v0").unwrap(), 0);
        assert_eq!(slot("v2").unwrap(), 2);
        assert_eq!(slot("slot2").unwrap(), 2);
        for invalid in ["v", "v-1", "v+1", "x2", "v2147483648"] {
            assert!(slot(invalid).is_err());
        }
    }
}

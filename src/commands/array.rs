use anyhow::ensure;

use crate::cli::Array;
use crate::jdwp::array_reference::{GetValues, Length, SetValues};
use crate::jdwp::value::Value;
use crate::wire;

use super::{Core, Data};

impl Core {
    pub(super) async fn array(&self, command: Array) -> anyhow::Result<Data> {
        match command {
            Array::Length { array } => {
                let length = self.client.request(Length { array }).await?;
                Ok(Data::ArrayLength {
                    array: wire::id(array),
                    length,
                })
            }
            Array::Get { array, range } => {
                let region = self
                    .client
                    .request(GetValues {
                        array,
                        start: range.start,
                        length: range.length,
                    })
                    .await?;

                Ok(Data::ArrayValues {
                    array: wire::id(array),
                    start: range.start,
                    tag: region.tag,
                    values: region.values.into_iter().map(Into::into).collect(),
                })
            }
            Array::Set {
                array,
                range,
                tag,
                values,
            } => {
                ensure!(
                    values.len() == range.length as usize,
                    "array range requires {} values, received {}",
                    range.length,
                    values.len()
                );

                let values = values
                    .iter()
                    .map(|value| Value::parse(tag, value))
                    .collect::<anyhow::Result<_>>()?;

                self.client
                    .request(SetValues {
                        array,
                        start: range.start,
                        values,
                    })
                    .await?;

                Ok(super::confirmed("array_set"))
            }
        }
    }
}

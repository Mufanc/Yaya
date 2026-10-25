use anyhow::{Context, bail, ensure};

use crate::cli::Object;
use crate::jdwp::class_type::SetValues as SetStaticValues;
use crate::jdwp::object_reference::{GetValues, ReferenceType, SetValues};
use crate::jdwp::reference_type::{GetValues as GetStaticValues, Instances, STATIC_MODIFIER};
use crate::jdwp::value::Value;
use crate::wire;

use super::Core;
use super::data::{Data, FieldValue, Member};

impl Core {
    pub(super) async fn object(&self, command: Object) -> anyhow::Result<Data> {
        match command {
            Object::Instances {
                class,
                type_id,
                limit,
            } => {
                ensure!(
                    limit > 0 && limit <= i32::MAX as u32,
                    "invalid instance limit"
                );

                let selected = self.resolve(&class, type_id).await?;
                let objects = self
                    .client
                    .request(Instances {
                        type_id: super::parse_id(&selected.type_id)?,
                        limit,
                    })
                    .await?
                    .into_iter()
                    .map(Into::into)
                    .collect();

                Ok(Data::Objects { objects })
            }

            Object::Inspect { object, fields } => {
                let members = self.object_fields(object).await?;
                let selected: Vec<_> = fields
                    .iter()
                    .map(|name| select_field(&members, name))
                    .collect::<anyhow::Result<_>>()?;
                let mut values = Vec::new();

                for field in selected {
                    let fields = vec![super::parse_id(&field.id)?];
                    let mut result = if field.modifiers & STATIC_MODIFIER != 0 {
                        self.client
                            .request(GetStaticValues {
                                type_id: super::parse_id(&field.declaring_type_id)?,
                                fields,
                            })
                            .await?
                    } else {
                        self.client.request(GetValues { object, fields }).await?
                    };

                    let value = result.pop().context("field reply count mismatch")?.into();

                    values.push(FieldValue { field, value });
                }

                Ok(Data::Fields { fields: values })
            }

            Object::Set {
                object,
                field,
                tag,
                value,
            } => {
                let value = Value::parse(tag, &value)?;
                let fields = self.object_fields(object).await?;
                let field = select_field(&fields, &field)?;
                let declared = field
                    .signature
                    .as_bytes()
                    .first()
                    .copied()
                    .context("empty field signature")?;

                ensure!(
                    tag == declared || (tag == b'L' && declared == b'['),
                    "value type does not match field signature {}",
                    field.signature
                );

                let fields = vec![(super::parse_id(&field.id)?, value)];

                if field.modifiers & STATIC_MODIFIER != 0 {
                    self.client
                        .request(SetStaticValues {
                            class: super::parse_id(&field.declaring_type_id)?,
                            fields,
                        })
                        .await?;
                } else {
                    self.client.request(SetValues { object, fields }).await?;
                }

                Ok(super::confirmed("object_set"))
            }
        }
    }

    async fn object_fields(&self, object: u64) -> anyhow::Result<Vec<Member>> {
        let object_type = self.client.request(ReferenceType { object }).await?;

        self.fields(
            self.type_info(object_type.type_id, object_type.type_tag)
                .await?,
        )
        .await
    }
}

fn select_field(fields: &[Member], selector: &str) -> anyhow::Result<Member> {
    let (declaring, name) = selector
        .split_once(':')
        .map_or((None, selector), |(id, name)| (Some(id), name));
    let declaring = declaring.map(super::parse_id).transpose()?;
    let matches: Vec<_> = fields
        .iter()
        .filter(|f| {
            f.name == name && declaring.is_none_or(|id| f.declaring_type_id == wire::id(id))
        })
        .collect();

    ensure!(!matches.is_empty(), "field not found: {selector}");

    if matches.len() != 1 {
        bail!(
            "ambiguous field {selector}; use declaring-type-id:name; candidates: {}",
            matches
                .iter()
                .map(|f| format!("{}:{}", f.declaring_type_id, f.name))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(matches[0].clone())
}

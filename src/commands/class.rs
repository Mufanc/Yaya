use std::collections::HashSet;

use anyhow::Context;

use crate::cli::Class;
use crate::jdwp::class_type::Superclass;
use crate::jdwp::reference_type::{
    CLASS_TAG, ClassLoader, Fields, INTERFACE_TAG, Interfaces, Member as ProtocolMember, Methods,
    Signature,
};
use crate::jdwp::virtual_machine::{AllClasses, ClassesBySignature};
use crate::jdwp::{Failure, TypeCandidate};
use crate::wire;

use super::Core;
use super::data::{Data, Member, TypeInfo};

impl Core {
    pub(super) async fn class(&self, command: Class) -> anyhow::Result<Data> {
        match command {
            Class::Prepare { command } => self.class_prepare(command).await,
            Class::Find { pattern } => {
                let classes = self.client.request(AllClasses).await?;
                let mut types = Vec::new();

                for class in classes {
                    if class.signature.contains(&pattern) {
                        types.push(TypeInfo {
                            type_tag: class.type_tag,
                            type_id: wire::id(class.type_id),
                            signature: class.signature,
                            status: class.status,
                            class_loader_id: wire::id(self.loader(class.type_id).await?),
                        });
                    }
                }

                Ok(Data::Types { types })
            }

            Class::Methods { class, type_id } => {
                let selected = self.resolve(&class, type_id).await?;
                let members = self.methods(&selected).await?;

                Ok(Data::Members { members })
            }

            Class::Fields { class, type_id } => {
                let selected = self.resolve(&class, type_id).await?;
                let members = self.fields(selected).await?;

                Ok(Data::Members { members })
            }
        }
    }

    async fn loader(&self, type_id: u64) -> anyhow::Result<u64> {
        Ok(self.client.request(ClassLoader { type_id }).await?.0)
    }

    pub(super) async fn resolve(
        &self,
        signature: &str,
        requested: Option<u64>,
    ) -> anyhow::Result<TypeInfo> {
        let classes = self
            .client
            .request(ClassesBySignature {
                signature: signature.into(),
            })
            .await?;
        let mut candidates = Vec::new();

        for class in classes {
            candidates.push(TypeInfo {
                type_tag: class.type_tag,
                type_id: wire::id(class.type_id),
                signature: signature.into(),
                status: class.status,
                class_loader_id: wire::id(self.loader(class.type_id).await?),
            });
        }

        if let Some(requested) = requested {
            return candidates
                .into_iter()
                .find(|candidate| candidate.type_id == wire::id(requested))
                .context("type-id does not match the loaded signature");
        }

        if candidates.len() > 1 {
            let mut error =
                Failure::new("ambiguous_type", format!("{signature}; specify --type-id"));

            error.candidates = candidates
                .iter()
                .map(|candidate| TypeCandidate {
                    type_id: candidate.type_id.clone(),
                    class_loader_id: candidate.class_loader_id.clone(),
                })
                .collect();

            return Err(error.into());
        }

        candidates
            .pop()
            .ok_or_else(|| Failure::new("class_not_loaded", signature).into())
    }

    pub(super) async fn methods(&self, selected: &TypeInfo) -> anyhow::Result<Vec<Member>> {
        let members = self
            .client
            .request(Methods {
                type_id: super::parse_id(&selected.type_id)?,
            })
            .await?;

        Ok(members
            .0
            .into_iter()
            .map(|member| member_output(member, selected))
            .collect())
    }

    pub(super) async fn fields(&self, selected: TypeInfo) -> anyhow::Result<Vec<Member>> {
        let mut remaining = vec![selected];
        let mut seen = HashSet::new();
        let mut fields = Vec::new();

        while let Some(selected) = remaining.pop() {
            if !seen.insert(selected.type_id.clone()) {
                continue;
            }

            let type_id = super::parse_id(&selected.type_id)?;
            let members = self.client.request(Fields { type_id }).await?;

            fields.extend(
                members
                    .0
                    .into_iter()
                    .map(|member| member_output(member, &selected)),
            );

            if selected.type_tag == CLASS_TAG {
                let parent = self.client.request(Superclass { class: type_id }).await?;

                if parent.0 != 0 {
                    remaining.push(self.type_info(parent.0, CLASS_TAG).await?);
                }
            }

            let interfaces = self.client.request(Interfaces { type_id }).await?;

            for interface in interfaces {
                remaining.push(self.type_info(interface.0, INTERFACE_TAG).await?);
            }
        }

        Ok(fields)
    }

    pub(super) async fn type_info(&self, type_id: u64, type_tag: u8) -> anyhow::Result<TypeInfo> {
        let signature = self.client.request(Signature { type_id }).await?;

        Ok(TypeInfo {
            type_tag,
            type_id: wire::id(type_id),
            signature,
            status: 0,
            class_loader_id: String::new(),
        })
    }
}

fn member_output(member: ProtocolMember, declaring: &TypeInfo) -> Member {
    Member {
        id: wire::id(member.id),
        name: member.name,
        signature: member.signature,
        modifiers: member.modifiers,
        declaring_type_id: declaring.type_id.clone(),
        declaring_class: declaring.signature.clone(),
    }
}

use anyhow::ensure;

use crate::cli::{Ddm, Ui};
use crate::jdwp::ddm::{self, Request};
use crate::wire;

use super::{CommandResult, Core, Data};

impl Core {
    pub(super) async fn ddm(&self, command: Ddm) -> anyhow::Result<Data> {
        match command {
            Ddm::Features => {
                let bytes = self
                    .client
                    .request(Request::Features)
                    .await?
                    .single("FEAT")?;

                Ok(Data::DdmFeatures {
                    features: ddm::strings(&bytes)?,
                })
            }
            Ddm::Raw { chunk, payload_hex } => {
                let payload = wire::unhex(&payload_hex)?;
                let chunks = self.client.request(Request::Raw { chunk, payload }).await?;

                Ok(Data::DdmRaw { chunks: chunks.0 })
            }
        }
    }

    pub(super) async fn ui(&self, command: Ui) -> anyhow::Result<CommandResult> {
        let data = match command {
            Ui::Windows => {
                let bytes = self
                    .client
                    .request(Request::Windows)
                    .await?
                    .single("VULW")?;

                anyhow::Ok(Data::Windows {
                    windows: ddm::strings(&bytes)?,
                })
            }
            Ui::Dump { window } => {
                let bytes = self
                    .client
                    .request(Request::Dump {
                        window: window.clone(),
                    })
                    .await?
                    .single("VURT")?;
                let hierarchy = ddm::hierarchy::decode(&bytes)?;

                anyhow::Ok(Data::ViewHierarchy { window, hierarchy })
            }
            Ui::Capture { window, view, .. } => {
                let png = self
                    .client
                    .request(Request::Capture { window, view })
                    .await?
                    .single("VUOP")?;

                ensure!(
                    png.starts_with(b"\x89PNG\r\n\x1a\n"),
                    "DDM capture did not return a PNG"
                );

                return Ok(CommandResult::File { bytes: png });
            }
        }?;

        Ok(CommandResult::Data { data })
    }
}

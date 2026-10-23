use crate::wire::{IdSizes, Reader, Writer};

#[derive(Debug)]
pub struct Location {
    pub type_tag: u8,
    pub type_id: u64,
    pub method_id: u64,
    pub code_index: u64,
}

impl Location {
    pub fn read(reader: &mut Reader<'_>, sizes: IdSizes) -> anyhow::Result<Self> {
        Ok(Self {
            type_tag: reader.read_u8()?,
            type_id: reader.read_id(sizes.reference_type)?,
            method_id: reader.read_id(sizes.method)?,
            code_index: reader.read_u64()?,
        })
    }

    pub fn write(&self, writer: &mut Writer, sizes: IdSizes) -> anyhow::Result<()> {
        writer.write_u8(self.type_tag);
        writer.write_id(self.type_id, sizes.reference_type)?;
        writer.write_id(self.method_id, sizes.method)?;
        writer.write_u64(self.code_index);

        Ok(())
    }
}

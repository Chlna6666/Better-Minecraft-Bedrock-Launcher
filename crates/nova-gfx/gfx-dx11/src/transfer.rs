use crate::device::{Dx11Device, backend, required};
use gfx_core::*;
use windows::Win32::Graphics::Direct3D11::*;

impl Dx11Device {
    pub(crate) fn readback(&mut self, id: TextureId) -> Result<TextureReadback> {
        let texture = self.textures.get(id)?;
        if !texture.desc.usage.contains(TextureUsage::COPY_SRC) {
            return Err(Error::InvalidInput("readback requires COPY_SRC".into()));
        }
        let mut staging = None;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: source is live; descriptor/output are writable owned locals.
        unsafe {
            texture.native.GetDesc(&mut desc);
        }
        desc.MipLevels = 1;
        desc.BindFlags = 0;
        desc.MiscFlags = 0;
        desc.Usage = D3D11_USAGE_STAGING;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        // SAFETY: staging descriptor has no bind flags and permits CPU readback.
        unsafe { self.native.CreateTexture2D(&desc, None, Some(&mut staging)) }.map_err(backend)?;
        let staging = required(staging)?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: both textures belong to the immediate context; Map waits for copy completion.
        unsafe {
            self.context
                .CopySubresourceRegion(&staging, 0, 0, 0, 0, &texture.native, 0, None);
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(backend)?;
        let row_bytes = texture.desc.size.width() * texture.desc.format.bytes_per_pixel();
        let mut bytes = vec![0u8; row_bytes as usize * texture.desc.size.height() as usize];
        for (row, destination) in bytes.chunks_exact_mut(row_bytes as usize).enumerate() {
            // SAFETY: mapped staging image covers Height rows of RowPitch bytes; copy only pixel bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    mapped
                        .pData
                        .cast::<u8>()
                        .add(row * mapped.RowPitch as usize),
                    destination.as_mut_ptr(),
                    destination.len(),
                );
            }
        }
        // SAFETY: successful Map above is paired with exactly one Unmap.
        unsafe {
            self.context.Unmap(&staging, 0);
        }
        Ok(TextureReadback {
            format: texture.desc.format,
            size: texture.desc.size,
            bytes_per_row: row_bytes,
            bytes,
        })
    }
}

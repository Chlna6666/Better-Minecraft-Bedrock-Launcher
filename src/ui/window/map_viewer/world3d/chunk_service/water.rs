//! Load animation strips once; frame selection belongs to the GPU water resolve.
use super::super::types::World3dWaterTexture;
use super::*;

impl Worker {
    pub(super) fn load_water_textures(&mut self) -> [Option<World3dWaterTexture>; 2] {
        let mut loaded = HashMap::<PathBuf, World3dWaterTexture>::new();
        [[0, 1, 0], [1, 0, 0]].map(|normal| {
            let source = self.resolver.texture_for("minecraft:water", normal)?;
            let key = source.source_path.path().to_path_buf();
            if let Some(texture) = loaded.get(&key) {
                return Some(texture.clone());
            }
            match self.load_water_strip(&source) {
                Ok(texture) => {
                    loaded.insert(key, texture.clone());
                    Some(texture)
                }
                Err(error) => {
                    self.last_error = Some(format!("准备水动画失败：{error}"));
                    None
                }
            }
        })
    }
    fn load_water_strip(
        &self,
        source: &bedrock_block_model::ObjResolvedTexture,
    ) -> anyhow::Result<World3dWaterTexture> {
        let bytes = source.source_path.read()?;
        // decode_texture_image retries color-mapped TGAs whose headers over-declare
        // palette alpha bits (vanilla water normal maps), which the TGA decoder rejects.
        let is_tga = source
            .source_path
            .path()
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("tga"));
        let image = bedrock_block_model::decode_texture_image(&bytes, is_tga)?.to_rgba8();
        let definition = self.resolver.flipbook_definition_for(source);
        let (image, seconds_per_frame) = animation_strip(image, definition)?;
        let asset = gpui_3d::TextureAsset::rgba8(image.width(), image.height(), image.into_raw())?;
        Ok(World3dWaterTexture {
            asset: Arc::new(asset),
            seconds_per_frame,
        })
    }
}

fn animation_strip(
    image: RgbaImage,
    definition: Option<&serde_json::Value>,
) -> anyhow::Result<(RgbaImage, f32)> {
    let width = image.width();
    let height = image.height();
    anyhow::ensure!(
        width > 0
            && height > 0
            && height % width == 0
            && width <= TEXTURE_MAX_DIMENSION
            && height <= TEXTURE_MAX_DIMENSION,
        "水动画必须为方形帧的垂直条带"
    );
    let ticks = definition
        .and_then(|v| v.get("ticks_per_frame"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1);
    anyhow::ensure!(ticks > 0 && ticks <= 1200, "水动画帧时长无效");
    let Some(frames) = definition
        .and_then(|v| v.get("frames"))
        .and_then(serde_json::Value::as_array)
    else {
        return Ok((image, ticks as f32 / 20.));
    };
    anyhow::ensure!(
        !frames.is_empty() && frames.len() <= (TEXTURE_MAX_DIMENSION / width) as usize,
        "水动画帧序列过长或为空"
    );
    let mut ordered = RgbaImage::new(width, width * frames.len() as u32);
    for (index, frame) in frames.iter().enumerate() {
        let row = frame
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("水动画目前要求等时数字帧序列"))?;
        anyhow::ensure!(row < u64::from(height / width), "水动画帧索引越界");
        let frame = image::imageops::crop_imm(&image, 0, row as u32 * width, width, width);
        image::imageops::replace(
            &mut ordered,
            &frame.to_image(),
            0,
            i64::from(index as u32 * width),
        );
    }
    Ok((ordered, ticks as f32 / 20.))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn water_strip_preserves_timing_order_and_rejects_bad_frames() {
        let image = RgbaImage::from_fn(2, 4, |_, y| {
            image::Rgba([
                if y < 2 { 255 } else { 0 },
                0,
                if y < 2 { 0 } else { 255 },
                255,
            ])
        });
        let definition = serde_json::json!({"ticks_per_frame":2,"frames":[1,0,1]});
        let (strip, seconds) = animation_strip(image.clone(), Some(&definition)).unwrap();
        assert_eq!((strip.width(), strip.height(), seconds), (2, 6, 0.1));
        assert_eq!(strip.get_pixel(0, 0).0, [0, 0, 255, 255]);
        assert_eq!(strip.get_pixel(0, 2).0, [255, 0, 0, 255]);
        assert!(animation_strip(image, Some(&serde_json::json!({"frames":[2]}))).is_err());
    }
}

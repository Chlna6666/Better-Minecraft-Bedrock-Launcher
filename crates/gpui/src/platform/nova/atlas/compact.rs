//! Bounded page repacking. Pixel storage remains on the GPU.
use super::*;

pub(in crate::platform::nova) struct AtlasCompactPlan {
    pub(in crate::platform::nova) source: NovaAtlasTextureInfo,
    pub(in crate::platform::nova) destination: NovaAtlasTextureInfo,
    pub(in crate::platform::nova) tiles: Vec<(AtlasTile, AtlasTile)>,
    allocator: BucketedAtlasAllocator,
    allocations: Vec<AllocId>,
}

impl NovaAtlas {
    pub(in crate::platform::nova) fn placement_generation(&self) -> u64 {
        self.placement_generation.load(AtomicOrdering::Acquire)
    }

    pub(in crate::platform::nova) fn copy_placements_into(
        &self,
        placements: &mut FxHashMap<crate::TileId, AtlasTile>,
    ) {
        let state = self.state.lock().expect("nova atlas lock poisoned");
        placements.clear();
        placements.extend(state.placements.iter().map(|(id, (tile, _))| (*id, *tile)));
    }

    /// Repack at most one sparse page. The callback must queue every copy before returning
    /// success, preserve source resources until those copies complete, and order future draws
    /// after the copies. On error, logical placements and page ownership remain unchanged.
    pub(in crate::platform::nova) fn compact_page(
        &self,
        copy: impl FnOnce(&AtlasCompactPlan) -> Result<()>,
    ) -> Result<bool> {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        let Some(plan) = state.compact_plan() else {
            return Ok(false);
        };
        copy(&plan)?;
        state.commit_compact(plan);
        self.publish_state_flags(&state);
        Ok(true)
    }
}

impl NovaAtlasState {
    fn compact_plan(&self) -> Option<AtlasCompactPlan> {
        // Pending upload bytes still address their original pages. Repacking starts only after
        // they have been queued, and never involves dedicated image pages or startup fallbacks.
        for list in &self.texture_lists {
            for source in list.textures.iter().flatten() {
                if source.dedicated
                    || source.size.width.0 <= NOVA_STARTUP_ATLAS_SIZE as i32
                    || self
                        .pending_uploads
                        .iter()
                        .any(|upload| upload.texture_id == source.id)
                {
                    continue;
                }
                let mut tiles: Vec<_> = self
                    .placements
                    .values()
                    .filter_map(|(tile, _)| (tile.texture_id == source.id).then_some(*tile))
                    .collect();
                let area: i64 = tiles
                    .iter()
                    .map(|tile| {
                        i64::from(tile.bounds.size.width.0 + 2 * tile.padding as i32)
                            * i64::from(tile.bounds.size.height.0 + 2 * tile.padding as i32)
                    })
                    .sum();
                if area == 0
                    || tiles.len() > 128
                    || area * 2 > i64::from(source.size.width.0) * i64::from(source.size.height.0)
                {
                    continue;
                }
                if area * atlas_bytes_per_pixel(source.id.kind) as i64 > 8 * 1024 * 1024 {
                    continue;
                }
                tiles.sort_unstable_by_key(|tile| std::cmp::Reverse(tile.bounds.size.height.0));
                // Trial allocations use a clone: an unsuccessful fit cannot disturb live tiles.
                for destination in list.textures.iter().flatten() {
                    if destination.id == source.id || destination.dedicated {
                        continue;
                    }
                    if let Some(plan) = repack(
                        source,
                        &tiles,
                        NovaAtlasTextureInfo {
                            id: destination.id,
                            size: destination.size,
                        },
                        destination.allocator.clone(),
                    ) {
                        return Some(plan);
                    }
                }
                let index = list
                    .free_list
                    .last()
                    .copied()
                    .unwrap_or(list.textures.len()) as u32;
                for axis in [512, 1024, 2048, 4096, 8192] {
                    if axis >= source.size.width.0 || axis >= source.size.height.0 {
                        break;
                    }
                    if let Some(plan) = repack(
                        source,
                        &tiles,
                        NovaAtlasTextureInfo {
                            id: AtlasTextureId {
                                index,
                                kind: source.id.kind,
                            },
                            size: crate::size(DevicePixels(axis), DevicePixels(axis)),
                        },
                        BucketedAtlasAllocator::new(etagere::Size::new(axis, axis)),
                    ) {
                        return Some(plan);
                    }
                }
            }
        }
        None
    }

    fn commit_compact(&mut self, plan: AtlasCompactPlan) {
        let list = &mut self.texture_lists[atlas_kind_index(plan.source.id.kind)];
        let destination = NovaAtlasTexture {
            id: plan.destination.id,
            size: plan.destination.size,
            allocator: plan.allocator,
            live_tile_count: plan.tiles.len(),
            dedicated: false,
        };
        let index = plan.destination.id.index as usize;
        if let Some(existing) = list.textures.get_mut(index).and_then(Option::as_mut) {
            existing.allocator = destination.allocator;
            existing.live_tile_count += destination.live_tile_count;
        } else if index == list.textures.len() {
            list.textures.push(Some(destination));
        } else {
            debug_assert_eq!(list.free_list.pop(), Some(index));
            list.textures[index] = Some(destination);
        }
        list.textures[plan.source.id.index as usize] = None;
        list.free_list.push(plan.source.id.index as usize);
        for ((_, tile), allocation) in plan.tiles.into_iter().zip(plan.allocations) {
            self.placements.insert(tile.tile_id, (tile, allocation));
        }
        // Cache hits and same-size refreshes also consume physical coordinates. Rewrite those
        // authoritative entries; immutable Scene copies resolve through logical IDs when packed.
        for tile in self
            .tiles
            .values_mut()
            .chain(self.fallback_tiles.iter_mut().flatten())
        {
            if let Some((placement, _)) = self.placements.get(&tile.tile_id) {
                *tile = *placement;
            }
        }
        for pending in &mut self.pending_removals {
            if let Some((placement, _)) = self.placements.get(&pending.tile.tile_id) {
                pending.tile = *placement;
            }
        }
        self.placement_generation = self.placement_generation.wrapping_add(1);
        self.texture_set_generation = self.texture_set_generation.wrapping_add(1);
        self.content_generation = self.content_generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sparse_atlas() -> (NovaAtlas, AtlasTile) {
        sparse_atlas_kind(AtlasTextureKind::Monochrome)
    }

    fn sparse_atlas_kind(kind: AtlasTextureKind) -> (NovaAtlas, AtlasTile) {
        let atlas = NovaAtlas::new();
        let mut state = atlas.state.lock().expect("atlas");
        *state = NovaAtlasState::default();
        let bytes = vec![255; 16 * 16 * atlas_bytes_per_pixel(kind)];
        let first = state
            .allocate_and_upload_kind(
                kind,
                crate::size(DevicePixels(16), DevicePixels(16)),
                &bytes,
            )
            .expect("first");
        let second = state
            .allocate_and_upload_kind(
                kind,
                crate::size(DevicePixels(16), DevicePixels(16)),
                &bytes,
            )
            .expect("second");
        state.deallocate_tile(first);
        state.pending_uploads.clear();
        drop(state);
        (atlas, second)
    }

    #[test]
    fn sparse_page_repacked_with_stable_identity_and_stale_handle_retirement() {
        let (atlas, old_tile) = sparse_atlas();
        assert!(
            atlas
                .compact_page(|plan| {
                    assert_eq!(plan.source.size.width.0, 2048);
                    assert_eq!(plan.destination.size.width.0, 512);
                    assert_eq!(plan.tiles[0].0.tile_id, plan.tiles[0].1.tile_id);
                    Ok(())
                })
                .expect("compact")
        );
        let mut placements = FxHashMap::default();
        atlas.copy_placements_into(&mut placements);
        let relocated = placements[&old_tile.tile_id];
        assert_ne!(old_tile.texture_id, relocated.texture_id);
        assert_eq!(old_tile.bounds.size, relocated.bounds.size);
        // The immutable old Scene handle deallocates the latest physical allocation.
        atlas.state.lock().expect("atlas").deallocate_tile(old_tile);
        atlas.copy_placements_into(&mut placements);
        assert!(!placements.contains_key(&old_tile.tile_id));
        assert!(atlas.texture_infos().is_empty());
    }

    #[test]
    fn failed_copy_does_not_publish_new_placement_or_release_source() {
        let (atlas, old_tile) = sparse_atlas();
        let generation = atlas.placement_generation();
        assert!(
            atlas
                .compact_page(|_| Err(anyhow::anyhow!("copy rejected")))
                .is_err()
        );
        let mut placements = FxHashMap::default();
        atlas.copy_placements_into(&mut placements);
        assert_eq!(placements[&old_tile.tile_id], old_tile);
        assert_eq!(atlas.placement_generation(), generation);
        assert_eq!(atlas.texture_infos()[0].id, old_tile.texture_id);
    }

    #[test]
    fn pending_upload_keeps_original_page_until_queued() {
        let (atlas, old_tile) = sparse_atlas();
        let mut state = atlas.state.lock().expect("atlas");
        state.enqueue_tile_upload_kind(
            old_tile.texture_id,
            AtlasTextureKind::Monochrome,
            old_tile.bounds.origin,
            old_tile.bounds.size,
            &[255; 256],
            old_tile.padding,
        );
        drop(state);
        assert!(
            !atlas
                .compact_page(|_| panic!("must not copy pending source uploads"))
                .expect("defer")
        );
    }

    #[test]
    fn compact_updates_cached_tile_before_same_size_refresh() {
        let (atlas, old_tile) = sparse_atlas_kind(AtlasTextureKind::Rgba);
        let key = AtlasKey::Image(crate::RenderImageParams {
            image_id: crate::ImageId(42),
            frame_slot: 0,
            pixel_format: crate::ImagePixelFormat::Rgba8,
        });
        atlas
            .state
            .lock()
            .expect("atlas")
            .tiles
            .insert(key.clone(), old_tile);
        atlas.compact_page(|_| Ok(())).expect("compact");
        let mut placements = FxHashMap::default();
        atlas.copy_placements_into(&mut placements);
        let relocated = placements[&old_tile.tile_id];
        let pixels = vec![255; 16 * 16 * 4];
        let tile = atlas
            .refresh_tile_with(&key, &mut || {
                Ok(Some((
                    old_tile.bounds.size,
                    std::borrow::Cow::Borrowed(&pixels),
                )))
            })
            .expect("refresh")
            .expect("tile");
        assert_eq!(tile, relocated);
        let state = atlas.state.lock().expect("atlas");
        let upload = state.pending_uploads.last().expect("pending refresh");
        assert_eq!(upload.texture_id, relocated.texture_id);
    }

    #[test]
    fn compact_prefers_existing_page_and_preserves_its_live_tiles() {
        let (atlas, old_tile) = sparse_atlas();
        let mut state = atlas.state.lock().expect("atlas");
        // Add a smaller, already resident page with one allocation. No new page is necessary.
        let list = &mut state.texture_lists[atlas_kind_index(AtlasTextureKind::Monochrome)];
        let id = AtlasTextureId {
            index: 1,
            kind: AtlasTextureKind::Monochrome,
        };
        let mut allocator = BucketedAtlasAllocator::new(etagere::Size::new(512, 512));
        let existing = allocator
            .allocate(etagere::Size::new(18, 18))
            .expect("existing tile");
        list.textures.push(Some(NovaAtlasTexture {
            id,
            size: crate::size(DevicePixels(512), DevicePixels(512)),
            allocator,
            live_tile_count: 1,
            dedicated: false,
        }));
        drop(state);
        assert!(
            atlas
                .compact_page(|plan| {
                    assert_eq!(plan.destination.id, id);
                    Ok(())
                })
                .expect("compact")
        );
        let mut state = atlas.state.lock().expect("atlas");
        let page = state.texture_lists[atlas_kind_index(id.kind)].textures[1]
            .as_mut()
            .expect("destination");
        assert_eq!(page.live_tile_count, 2);
        page.allocator.deallocate(existing.id);
        state.deallocate_tile(old_tile);
        assert_eq!(
            state.texture_lists[atlas_kind_index(id.kind)].textures[1]
                .as_ref()
                .expect("existing page")
                .live_tile_count,
            1
        );
    }
}

fn repack(
    source: &NovaAtlasTexture,
    tiles: &[AtlasTile],
    destination: NovaAtlasTextureInfo,
    mut allocator: BucketedAtlasAllocator,
) -> Option<AtlasCompactPlan> {
    let mut moves = Vec::with_capacity(tiles.len());
    let mut allocations = Vec::with_capacity(tiles.len());
    for tile in tiles {
        let padding = tile.padding as i32;
        let allocation = allocator.allocate(etagere::Size::new(
            tile.bounds.size.width.0 + 2 * padding,
            tile.bounds.size.height.0 + 2 * padding,
        ))?;
        let mut relocated = *tile;
        relocated.texture_id = destination.id;
        relocated.bounds.origin = crate::point(
            DevicePixels(allocation.rectangle.min.x + padding),
            DevicePixels(allocation.rectangle.min.y + padding),
        );
        moves.push((*tile, relocated));
        allocations.push(allocation.id);
    }
    Some(AtlasCompactPlan {
        source: NovaAtlasTextureInfo {
            id: source.id,
            size: source.size,
        },
        destination,
        tiles: moves,
        allocator,
        allocations,
    })
}

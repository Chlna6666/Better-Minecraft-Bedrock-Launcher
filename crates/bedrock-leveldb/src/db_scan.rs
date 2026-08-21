impl Db {
    pub(super) fn for_each_key_scan<F>(
        &self,
        options: ReadOptions,
        mut visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
    {
        self.scan_visible_borrowed(None, &options, &mut |key, _value| visitor(key))
    }

    pub(super) fn for_each_entry_borrowed_scan<F>(
        &self,
        options: ReadOptions,
        mut visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &[u8]) -> Result<VisitorControl> + Send,
    {
        self.scan_visible_borrowed(None, &options, &mut visitor)
    }

    pub(super) fn for_each_prefix_borrowed_scan<F>(
        &self,
        prefix: &[u8],
        options: ReadOptions,
        mut visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &[u8]) -> Result<VisitorControl> + Send,
    {
        self.scan_visible_borrowed(Some(prefix), &options, &mut visitor)
    }

    pub(super) fn for_each_prefix_key_scan<F>(
        &self,
        prefix: &[u8],
        options: ReadOptions,
        mut visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
    {
        self.scan_visible_borrowed(Some(prefix), &options, &mut |key, _value| visitor(key))
    }

    pub(super) fn scan_keys_partitioned_scan<T, I, F>(
        &self,
        options: ReadOptions,
        init: I,
        visitor: F,
    ) -> Result<(ScanOutcome, Vec<T>)>
    where
        T: Send,
        I: Fn() -> T + Send + Sync,
        F: Fn(&mut T, &[u8]) -> Result<VisitorControl> + Send + Sync,
    {
        let state = self.read_state_snapshot()?;
        let tables = state
            .version
            .tables()
            .iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>();
        let active = Arc::clone(&state.active);
        let immutable = state.immutable.as_ref().map(Arc::clone);
        let shadowed = |key: &[u8]| {
            active.contains_key(key)
                || immutable
                    .as_ref()
                    .is_some_and(|table| table.entries().contains_key(key))
        };
        let verify_checksums = read_checksums(&self.shared.options, &options);
        let (mut outcome, mut partitions) = crate::table_scan::scan_table_keys_partitioned(
            &self.shared.root,
            &tables,
            None,
            verify_checksums,
            &options,
            shadowed,
            &init,
            &visitor,
        )?;

        if outcome.stopped {
            return Ok((outcome, partitions));
        }
        if partitions.is_empty() {
            partitions.push(init());
        }
        let partition = partitions
            .first_mut()
            .expect("partition was inserted when table scan returned none");

        if let Some(immutable) = &state.immutable {
            for (key, value) in immutable.entries() {
                check_visibility_scan_cancelled(&options, &mut outcome)?;
                if state.active.contains_key(key) {
                    continue;
                }
                if let Some(value) = value {
                    outcome.record(value.len());
                    if visitor(partition, key)? == VisitorControl::Stop {
                        outcome.stopped = true;
                        return Ok((outcome, partitions));
                    }
                    emit_visibility_scan_progress(&options, &outcome);
                }
            }
        }
        for (key, value) in state.active.iter() {
            check_visibility_scan_cancelled(&options, &mut outcome)?;
            if let Some(value) = value {
                outcome.record(value.len());
                if visitor(partition, key)? == VisitorControl::Stop {
                    outcome.stopped = true;
                    return Ok((outcome, partitions));
                }
                emit_visibility_scan_progress(&options, &outcome);
            }
        }
        Ok((outcome, partitions))
    }

    fn scan_visible_borrowed<F>(
        &self,
        prefix: Option<&[u8]>,
        options: &ReadOptions,
        visitor: &mut F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &[u8]) -> Result<VisitorControl> + Send,
    {
        let started = Instant::now();
        let state = self.read_state_snapshot()?;
        let tables = state
            .version
            .tables()
            .iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>();
        let active = Arc::clone(&state.active);
        let immutable = state.immutable.as_ref().map(Arc::clone);
        let shadowed = |key: &[u8]| {
            active.contains_key(key)
                || immutable
                    .as_ref()
                    .is_some_and(|table| table.entries().contains_key(key))
        };
        let verify_checksums = read_checksums(&self.shared.options, options);
        let mut outcome = crate::table_scan::scan_tables_visible(
            &self.shared.root,
            &tables,
            prefix,
            verify_checksums,
            options,
            shadowed,
            visitor,
        )?;
        if outcome.stopped {
            return Ok(outcome);
        }

        if let Some(immutable) = &state.immutable {
            for (key, value) in immutable.entries() {
                check_visibility_scan_cancelled(options, &mut outcome)?;
                if state.active.contains_key(key)
                    || prefix.is_some_and(|prefix| !key.starts_with(prefix))
                {
                    continue;
                }
                if let Some(value) = value {
                    outcome.record(value.len());
                    if visitor(key, value.as_ref())? == VisitorControl::Stop {
                        outcome.stopped = true;
                        return Ok(outcome);
                    }
                    emit_visibility_scan_progress(options, &outcome);
                }
            }
        }
        for (key, value) in state.active.iter() {
            check_visibility_scan_cancelled(options, &mut outcome)?;
            if prefix.is_some_and(|prefix| !key.starts_with(prefix)) {
                continue;
            }
            if let Some(value) = value {
                outcome.record(value.len());
                if visitor(key, value.as_ref())? == VisitorControl::Stop {
                    outcome.stopped = true;
                    return Ok(outcome);
                }
                emit_visibility_scan_progress(options, &outcome);
            }
        }
        outcome.worker_threads = outcome.worker_threads.max(1);
        log::debug!(
            "borrowed visibility scan complete (visited={}, tables={}, workers={}, elapsed_ms={})",
            outcome.visited,
            outcome.tables_scanned,
            outcome.worker_threads,
            started.elapsed().as_millis()
        );
        Ok(outcome)
    }
}

fn check_visibility_scan_cancelled(
    options: &ReadOptions,
    outcome: &mut ScanOutcome,
) -> Result<()> {
    outcome.cancel_checks = outcome.cancel_checks.saturating_add(1);
    if options
        .cancel
        .as_ref()
        .is_some_and(|cancel| cancel.is_cancelled())
    {
        return Err(LevelDbError::cancelled("database scan"));
    }
    Ok(())
}

fn emit_visibility_scan_progress(options: &ReadOptions, outcome: &ScanOutcome) {
    let interval = options.pipeline.resolve_progress_interval().max(1);
    if outcome.visited != 0
        && outcome.visited.is_multiple_of(interval)
        && let Some(progress) = &options.progress
    {
        progress.emit(crate::options::ScanProgress {
            visited: outcome.visited,
            bytes_read: outcome.bytes_read,
        });
    }
}

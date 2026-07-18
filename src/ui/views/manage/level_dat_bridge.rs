use super::*;
use crate::ui::views::manage::level_dat_schema;

impl ManagePageView {
    pub fn close_level_dat_editor(&mut self, cx: &mut Context<Self>) {
        self.level_dat_editor = None;
        self.value_prompt = None;
        navigate_to_manage_root(cx);
        cx.notify();
    }

    pub fn return_from_level_dat_editor(&mut self, cx: &mut Context<Self>) {
        self.value_prompt = None;
        navigate_to_level_dat_editor_host(cx, false);
        cx.notify();
    }

    pub fn resume_level_dat_editor(&mut self, cx: &mut Context<Self>) {
        if self.level_dat_editor.is_none() {
            return;
        }
        navigate_to_level_dat_editor_host(cx, true);
        cx.notify();
    }

    pub fn set_level_dat_editor_mode(
        &mut self,
        mode: level_dat_editor::LevelDatEditorMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(current_mode) = self.level_dat_editor.as_ref().map(|editor| editor.mode) else {
            return;
        };
        if current_mode == mode {
            return;
        }

        match mode {
            level_dat_editor::LevelDatEditorMode::Visual => {
                let Some(editor) = self.level_dat_editor.as_ref() else {
                    return;
                };
                let editor_text = editor.json_editor.read(cx).value();
                let parsed_root = match level_dat_editor::parse_document_json(editor_text.as_ref())
                {
                    Ok(root) => root,
                    Err(validation) => {
                        if let Some(editor) = self.level_dat_editor.as_mut() {
                            editor.validation = validation.clone();
                        }
                        toast::error(
                            cx,
                            validation
                                .detail
                                .clone()
                                .unwrap_or_else(|| validation.summary.clone()),
                        );
                        cx.notify();
                        return;
                    }
                };

                if let Some(editor) = self.level_dat_editor.as_mut() {
                    editor.document.root = parsed_root;
                    editor.validation =
                        level_dat_editor::validate_document_json(editor_text.as_ref());
                    editor.visual_dirty = editor_text != editor.saved_text;
                }
                self.sync_level_dat_form_inputs(window, cx);
            }
            level_dat_editor::LevelDatEditorMode::Json => {
                if let Err(error) = self.commit_level_dat_form_inputs(cx) {
                    toast::error(cx, SharedString::from(error));
                    return;
                }
            }
        }

        if let Some(editor) = self.level_dat_editor.as_mut() {
            editor.mode = mode;
        }
        cx.notify();
    }

    pub(super) fn sync_level_dat_json_from_document(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(editor) = self.level_dat_editor.as_mut() else {
            return Ok(());
        };
        let json_text = level_dat_editor::document_to_json_text(&editor.document)?;
        editor.json_editor.update(cx, |code_editor, cx| {
            code_editor.set_value(json_text.clone(), cx);
        });
        editor.validation = level_dat_editor::validate_document_json(json_text.as_ref());
        Ok(())
    }

    pub(super) fn sync_level_dat_form_field(
        &mut self,
        field: level_dat_editor::ValueFieldSpec,
        value: &str,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        let Some(editor) = self.level_dat_editor.as_ref() else {
            return Ok(false);
        };
        let mut next_document = editor.document.clone();
        match level_dat_editor::apply_value_text(&mut next_document, field, value) {
            Ok(()) => {
                if let Some(editor) = self.level_dat_editor.as_mut() {
                    editor.document = next_document;
                    editor.visual_dirty = true;
                }
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn handle_level_dat_form_input_event(
        &mut self,
        field: level_dat_editor::ValueFieldSpec,
        input: &Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let value = input.read(cx).value().to_string();
                let result = match field.kind {
                    level_dat_editor::ValueFieldKind::String => {
                        self.sync_level_dat_form_field(field, &value, cx)
                    }
                    _ => {
                        let trimmed = value.trim();
                        if trimmed.is_empty() {
                            self.sync_level_dat_form_field(field, &value, cx)
                        } else {
                            match self.sync_level_dat_form_field(field, &value, cx) {
                                Ok(changed) => Ok(changed),
                                Err(_) => Ok(false),
                            }
                        }
                    }
                };
                match result {
                    Ok(true) => cx.notify(),
                    Ok(false) => {}
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                }
            }
            InputEvent::PressEnter { .. } => {
                let value = input.read(cx).value().to_string();
                match self.sync_level_dat_form_field(field, &value, cx) {
                    Ok(true) => cx.notify(),
                    Ok(false) => {}
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                }
            }
            InputEvent::Focus | InputEvent::Blur => {}
        }
    }

    pub(super) fn commit_level_dat_form_inputs(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(editor) = self.level_dat_editor.as_ref() else {
            return Ok(());
        };
        let fields = level_dat_editor::form_value_fields(&editor.document);
        let mut values = Vec::with_capacity(fields.len());
        for field in fields {
            let key = level_dat_editor::form_input_key(field);
            let Some(input) = editor.form_inputs.get(&key) else {
                continue;
            };
            values.push((field, input.read(cx).value().to_string()));
        }

        let Some(editor) = self.level_dat_editor.as_ref() else {
            return Ok(());
        };
        let mut next_document = editor.document.clone();
        for (field, value) in values {
            level_dat_editor::apply_value_text(&mut next_document, field, &value)?;
        }

        if let Some(editor) = self.level_dat_editor.as_mut() {
            editor.document = next_document;
        }
        self.sync_level_dat_json_from_document(cx)
    }

    pub fn toggle_level_dat_field(
        &mut self,
        field: level_dat_editor::BoolFieldSpec,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.level_dat_editor.as_mut() {
            level_dat_editor::toggle_bool(&mut editor.document, field);
            editor.visual_dirty = true;
        }

        cx.notify();
    }

    pub fn set_level_dat_choice(
        &mut self,
        field: level_dat_editor::ChoiceFieldSpec,
        value: i32,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.level_dat_editor.as_mut() {
            level_dat_editor::set_choice_value(&mut editor.document, field, value);
            editor.visual_dirty = true;
        }
        cx.notify();
    }

    pub fn toggle_level_dat_group(
        &mut self,
        group: level_dat_editor::LevelDatFieldGroup,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.level_dat_editor.as_mut() else {
            return;
        };
        if !editor.collapsed_groups.remove(&group) {
            editor.collapsed_groups.insert(group);
        }
        cx.notify();
    }

    pub fn open_level_dat_value_prompt(
        &mut self,
        field: level_dat_editor::ValueFieldSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.level_dat_editor.as_ref() else {
            return;
        };
        let Some(input) = create_text_input(
            window,
            cx,
            field.key,
            level_dat_editor::value_text(&editor.document, field).as_ref(),
        ) else {
            return;
        };
        self.value_prompt = Some(ValuePromptDialogState {
            title: SharedString::from(field.label),
            description: SharedString::from(field.key),
            confirm_label: SharedString::from("应用"),
            input,
            target: ValuePromptTarget::LevelDat(field),
            pending: false,
        });
        cx.notify();
    }

    pub fn save_level_dat_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.level_dat_editor.as_ref() else {
            return;
        };
        if editor.saving {
            return;
        }

        let (document, saved_text) = match editor.mode {
            level_dat_editor::LevelDatEditorMode::Visual => {
                if let Err(error) = self.commit_level_dat_form_inputs(cx) {
                    toast::error(cx, SharedString::from(error));
                    return;
                }
                let Some(editor) = self.level_dat_editor.as_ref() else {
                    return;
                };
                (editor.document.clone(), editor.json_editor.read(cx).value())
            }
            level_dat_editor::LevelDatEditorMode::Json => {
                let editor_text = editor.json_editor.read(cx).value();
                let parsed_root = match level_dat_editor::parse_document_json(editor_text.as_ref())
                {
                    Ok(root) => root,
                    Err(validation) => {
                        if let Some(editor) = self.level_dat_editor.as_mut() {
                            editor.validation = validation.clone();
                        }
                        toast::error(
                            cx,
                            validation
                                .detail
                                .clone()
                                .unwrap_or_else(|| validation.summary.clone()),
                        );
                        cx.notify();
                        return;
                    }
                };

                if let Some(editor) = self.level_dat_editor.as_mut() {
                    editor.document.root = parsed_root;
                    editor.validation =
                        level_dat_editor::validate_document_json(editor_text.as_ref());
                    editor.needs_form_sync = true;
                }
                let Some(editor) = self.level_dat_editor.as_ref() else {
                    return;
                };
                (editor.document.clone(), editor_text)
            }
        };

        let Some(editor) = self.level_dat_editor.as_mut() else {
            return;
        };
        editor.saving = true;
        let folder_path = editor.asset.file_path.to_string();
        cx.spawn(async move |handle, cx| {
            let result = crate::tasks::runtime::run_blocking(
                crate::tasks::runtime::BlockingTaskOptions::hidden("保存 level.dat"),
                move || {
                    let world_path = std::path::PathBuf::from(&folder_path);
                    let history_capture =
                        crate::ui::window::map_viewer::map_history::capture_before(
                            crate::ui::window::map_viewer::map_history::MapHistoryCaptureSpec {
                                kind: crate::ui::window::map_viewer::map_history::MapHistoryEntryKind::LevelDatSave,
                                label: "保存 level.dat".to_string(),
                                world_path: world_path.clone(),
                                chunks: std::collections::BTreeSet::new(),
                                raw_keys: std::collections::BTreeSet::new(),
                                include_level_dat: true,
                            },
                        );
                    let result = data::write_level_dat_document(&folder_path, &document);
                    match (history_capture, result) {
                        (Ok(capture), Ok(())) => {
                            crate::ui::window::map_viewer::map_history::complete_after(
                                capture,
                                "level.dat 已保存",
                            )?;
                            Ok(())
                        }
                        (Ok(capture), Err(error)) => {
                            if let Err(history_error) =
                                crate::ui::window::map_viewer::map_history::complete_failed(
                                    capture,
                                    error.clone(),
                                )
                            {
                                tracing::warn!(%history_error, "map history failure recording failed");
                            }
                            Err(error)
                        }
                        (Err(error), Ok(())) => {
                            tracing::warn!(%error, "map history capture failed after level.dat save");
                            Ok(())
                        }
                        (Err(history_error), Err(write_error)) => {
                            Err(format!("{write_error}；历史捕获失败: {history_error}"))
                        }
                    }
                },
            )
                .await;

            let _ = handle.update(cx, |this, cx| {
                if let Some(editor) = this.level_dat_editor.as_mut() {
                    editor.saving = false;
                }
                match result {
                    Ok(()) => {
                        if let Some(editor) = this.level_dat_editor.as_mut() {
                            editor.saved_text = saved_text.clone();
                            editor.validation =
                                level_dat_editor::validate_document_json(saved_text.as_ref());
                            editor.visual_dirty = false;
                        }
                        toast::success(cx, SharedString::from("地图数据已保存"));
                    }
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                }
                cx.notify();
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub fn format_level_dat_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.level_dat_editor.as_ref() else {
            return;
        };
        let editor_text = editor.json_editor.read(cx).value();
        let parsed_root = match level_dat_editor::parse_document_json(editor_text.as_ref()) {
            Ok(root) => root,
            Err(validation) => {
                if let Some(editor) = self.level_dat_editor.as_mut() {
                    editor.validation = validation.clone();
                }
                toast::error(
                    cx,
                    validation
                        .detail
                        .clone()
                        .unwrap_or_else(|| validation.summary.clone()),
                );
                cx.notify();
                return;
            }
        };

        let formatted = match level_dat_editor::format_document_json(&parsed_root) {
            Ok(text) => text,
            Err(error) => {
                toast::error(cx, SharedString::from(error));
                return;
            }
        };

        if let Some(editor) = self.level_dat_editor.as_mut() {
            editor.json_editor.update(cx, |code_editor, cx| {
                code_editor.set_value(formatted.clone(), cx);
            });
            editor.document.root = parsed_root;
            editor.validation = level_dat_editor::validate_document_json(formatted.as_ref());
            editor.needs_form_sync = true;
        }
        toast::success(cx, SharedString::from("JSON 已格式化"));
        cx.notify();
    }

    pub fn revalidate_level_dat_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.level_dat_editor.as_mut() else {
            return;
        };
        let editor_text = editor.json_editor.read(cx).value();
        editor.validation = level_dat_editor::validate_document_json(editor_text.as_ref());
        cx.notify();
    }

    pub fn launch_level_map_from_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.level_dat_editor.as_ref() else {
            return;
        };
        launch_map_version(&editor.version, &editor.asset, cx);
    }

    pub fn open_level_dat_editor_code_window(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.level_dat_editor.as_ref() else {
            return;
        };
        if editor.mode == level_dat_editor::LevelDatEditorMode::Visual {
            if let Err(error) = self.commit_level_dat_form_inputs(cx) {
                toast::error(cx, SharedString::from(error));
                return;
            }
        }

        let Some(editor) = self.level_dat_editor.as_ref() else {
            return;
        };

        let initial_text = editor.json_editor.read(cx).value();
        crate::ui::window::level_dat::open_level_dat_code_window(
            crate::ui::window::level_dat::LevelDatCodeWindowInit {
                version: editor.version.clone(),
                asset: editor.asset.clone(),
                document_version: editor.document.version(),
                initial_text,
                saved_text: editor.saved_text.clone(),
            },
            cx,
        );
    }

    pub(super) fn sync_level_dat_form_inputs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.level_dat_editor.as_ref() else {
            return;
        };
        let document = editor.document.clone();
        let fields = level_dat_editor::form_value_fields(&document);
        let mut created_inputs = HashMap::new();
        let mut existing_updates = Vec::new();

        for field in fields {
            let key = level_dat_editor::form_input_key(field);
            let current_input = self
                .level_dat_editor
                .as_ref()
                .and_then(|editor| editor.form_inputs.get(&key).cloned());

            if let Some(input) = current_input {
                existing_updates.push((input, level_dat_editor::value_text(&document, field)));
                continue;
            }

            let placeholder = level_dat_editor::input_placeholder(field).to_string();
            let initial_value = level_dat_editor::value_text(&document, field);
            let input = cx.new(|cx| {
                let mut input_state = InputState::new(window, cx);
                input_state.set_placeholder(SharedString::from(placeholder), window, cx);
                if !initial_value.is_empty() {
                    input_state.set_value(initial_value.clone(), window, cx);
                }
                input_state
            });

            let current_key = key.clone();
            let subscription = cx.subscribe(&input, move |this, input, event: &InputEvent, cx| {
                let Some(active_input) = this
                    .level_dat_editor
                    .as_ref()
                    .and_then(|editor| editor.form_inputs.get(&current_key))
                else {
                    return;
                };
                if active_input.entity_id() != input.entity_id() {
                    return;
                }
                this.handle_level_dat_form_input_event(field, &input, event, cx);
            });
            self._subscriptions.push(subscription);
            created_inputs.insert(key, input);
        }

        if let Some(editor) = self.level_dat_editor.as_mut() {
            editor.form_inputs.extend(created_inputs);
        }

        for (input, value) in existing_updates {
            input.update(cx, |input_state, cx| {
                input_state.set_value(value.clone(), window, cx);
            });
        }
    }
    pub(super) fn open_level_dat_editor(
        &mut self,
        asset: ManageAssetEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = cx.global::<ManagePageState>();
        let Some(version) = self.selected_version(state).cloned() else {
            return;
        };
        let folder_path = asset.file_path.to_string();
        cx.spawn_in(window, async move |handle, cx| {
            let result = crate::tasks::runtime::run_blocking(
                crate::tasks::runtime::BlockingTaskOptions::hidden("读取 level.dat"),
                move || data::read_level_dat_document(&folder_path),
            )
            .await;

            let _ = handle.update_in(cx, |this, window, cx| {
                match result {
                    Ok(document) => {
                        let json_text = match level_dat_editor::document_to_json_text(&document) {
                            Ok(text) => text,
                            Err(error) => {
                                toast::error(cx, SharedString::from(error));
                                return;
                            }
                        };
                        let validation =
                            level_dat_editor::validate_document_json(json_text.as_ref());
                        let json_editor = cx.new(|cx| {
                            let mut editor =
                                crate::ui::components::code_editor::CodeEditorState::new(cx);
                            editor.set_language(CodeEditorLanguage::JsonNbt, cx);
                            editor.set_value(json_text.clone(), cx);
                            editor
                        });
                        let subscription = cx.subscribe(
                            &json_editor,
                            |this, editor, event: &CodeEditorEvent, cx| {
                                let Some(current_editor_id) = this
                                    .level_dat_editor
                                    .as_ref()
                                    .map(|level_editor| level_editor.json_editor.entity_id())
                                else {
                                    return;
                                };
                                if current_editor_id != editor.entity_id() {
                                    return;
                                }
                                match event {
                                    CodeEditorEvent::Change => {
                                        this.revalidate_level_dat_editor(cx);
                                    }
                                    CodeEditorEvent::SaveRequested => {
                                        this.save_level_dat_editor(cx);
                                    }
                                    CodeEditorEvent::FormatRequested => {
                                        this.format_level_dat_editor(cx);
                                    }
                                    CodeEditorEvent::PointerInteractionStarted
                                    | CodeEditorEvent::PointerInteractionEnded => {}
                                }
                            },
                        );
                        this._subscriptions.push(subscription);
                        this.level_dat_editor = Some(level_dat_editor::LevelDatEditorModalState {
                            version,
                            asset,
                            document,
                            json_editor,
                            mode: level_dat_editor::LevelDatEditorMode::Visual,
                            validation,
                            saved_text: json_text,
                            saving: false,
                            form_inputs: HashMap::new(),
                            needs_form_sync: false,
                            visual_dirty: false,
                            collapsed_groups: level_dat_schema::default_collapsed_groups()
                                .into_iter()
                                .collect(),
                        });
                        this.sync_level_dat_form_inputs(window, cx);
                        navigate_to_level_dat_editor_host(cx, true);
                    }
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                }
                cx.notify();
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}

pub(super) fn is_level_dat_editor_route(cx: &App) -> bool {
    gpui_router::use_location(cx)
        .pathname
        .starts_with(LEVEL_DAT_EDITOR_ROUTE_PATH)
}

pub(super) fn navigate_to_level_dat_editor_host(cx: &mut App, open_editor: bool) {
    let mut navigate = gpui_router::use_navigate(cx);
    navigate(
        if open_editor {
            LEVEL_DAT_EDITOR_ROUTE_PATH
        } else {
            AppRoute::Manage.pathname()
        }
        .into(),
    );
}

pub(super) fn navigate_to_manage_root(cx: &mut App) {
    navigate_to_level_dat_editor_host(cx, false);
}

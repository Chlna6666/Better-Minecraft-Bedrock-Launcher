from pathlib import Path
import runpy

path = Path(__file__).with_name("apply-map-entity-player-editor-v3.py")
text = path.read_text(encoding="utf-8")

# Scope the context-menu replacement to open_context_menu; the same chrome reset
# sequence also appears in another interaction path.
old = '''text = replace_once(
    text,
    ''' + "'''" + '''        self.ui_state.top_more_open = false;\n        self.ui_state.context_more_open = false;''' + "'''" + ''',
    ''' + "'''" + '''        self.ui_state.top_more_open = false;\n        self.players.context_target = None;\n        self.professional.entity_context_target = self.entity_context_target_at(position);\n        self.ui_state.context_more_open = false;''' + "'''" + ''',
    "set entity context target",
)
'''
new = '''open_context_pos = text.find(open_marker)
if open_context_pos < 0:
    raise RuntimeError("open context menu marker missing")
close_context_pos = text.find("    pub(super) fn close_context_menu", open_context_pos)
if close_context_pos < 0:
    raise RuntimeError("close context menu marker missing")
open_context = text[open_context_pos:close_context_pos]
open_context = replace_once(
    open_context,
    ''' + "'''" + '''        self.ui_state.top_more_open = false;\n        self.ui_state.context_more_open = false;''' + "'''" + ''',
    ''' + "'''" + '''        self.ui_state.top_more_open = false;\n        self.players.context_target = None;\n        self.professional.entity_context_target = self.entity_context_target_at(position);\n        self.ui_state.context_more_open = false;''' + "'''" + ''',
    "set entity context target",
)
text = text[:open_context_pos] + open_context + text[close_context_pos:]
'''
if text.count(old) != 1:
    raise RuntimeError(f"runner expected one context patch block, got {text.count(old)}")
text = text.replace(old, new, 1)

# The generic edit completion path now uses a fixed status string rather than a
# precomputed operation/message variable. Patch the same semantics against the
# current run_confirmed_edit implementation.
old = '''text = replace_once(
    text,
    ''' + "'''" + '''        let operation = edit_action_status(&action, &target);\n        let history_spec = match edit_history_spec(&self.world_path, &target, &action) {''' + "'''" + ''',
    ''' + "'''" + '''        let operation = edit_action_status(&action, &target);\n        let deletes_player = matches!((&target, &action), (EditTarget::Player(_), EditAction::Delete));\n        let history_spec = match edit_history_spec(&self.world_path, &target, &action) {''' + "'''" + ''',
    "player delete completion flag",
)
'''
new = '''text = replace_once(
    text,
    ''' + "'''" + '''        let document_text = matches!(action, EditAction::Save)\n            .then(|| self.editor_state.read(cx).value().to_string());\n        self.status =''' + "'''" + ''',
    ''' + "'''" + '''        let document_text = matches!(action, EditAction::Save)\n            .then(|| self.editor_state.read(cx).value().to_string());\n        let deletes_player =\n            matches!((&target, &action), (EditTarget::Player(_), EditAction::Delete));\n        self.status =''' + "'''" + ''',
    "player delete completion flag",
)
'''
if text.count(old) != 1:
    raise RuntimeError(f"runner expected one player completion patch block, got {text.count(old)}")
text = text.replace(old, new, 1)

old = '''text = replace_once(
    text,
    ''' + "'''" + '''                                this.apply_map_edit_invalidation(&invalidation, cx);\n                                this.status = SharedString::from(message.clone());''' + "'''" + ''',
    ''' + "'''" + '''                                this.apply_map_edit_invalidation(&invalidation, cx);\n                                if deletes_player {\n                                    this.players.selected = None;\n                                    this.players.detail = None;\n                                    this.players.context_target = None;\n                                    this.refresh_players(cx);\n                                }\n                                this.status = SharedString::from(message.clone());''' + "'''" + ''',
    "refresh players after delete",
)
'''
new = '''text = replace_once(
    text,
    ''' + "'''" + '''                        this.apply_map_edit_invalidation(&invalidation, cx);\n                        this.status = SharedString::from("世界记录已写入并刷新地图状态");''' + "'''" + ''',
    ''' + "'''" + '''                        this.apply_map_edit_invalidation(&invalidation, cx);\n                        if deletes_player {\n                            this.players.selected = None;\n                            this.players.detail = None;\n                            this.players.context_target = None;\n                            this.refresh_players(cx);\n                        }\n                        this.status = SharedString::from("世界记录已写入并刷新地图状态");''' + "'''" + ''',
    "refresh players after delete",
)
'''
if text.count(old) != 1:
    raise RuntimeError(f"runner expected one refresh-player patch block, got {text.count(old)}")
text = text.replace(old, new, 1)

path.write_text(text, encoding="utf-8")
runpy.run_path(str(path), run_name="__main__")

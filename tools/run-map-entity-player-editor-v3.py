from pathlib import Path
import runpy

path = Path(__file__).with_name("apply-map-entity-player-editor-v3.py")
text = path.read_text(encoding="utf-8")
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
count = text.count(old)
if count != 1:
    raise RuntimeError(f"runner expected one failing source block, got {count}")
path.write_text(text.replace(old, new, 1), encoding="utf-8")
runpy.run_path(str(path), run_name="__main__")

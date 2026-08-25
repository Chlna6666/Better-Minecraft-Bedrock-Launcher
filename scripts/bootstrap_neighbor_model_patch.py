from pathlib import Path

path = Path(__file__).with_name("fix_neighbor_dependent_models.py")
source = path.read_text(encoding="utf-8")
old = "    '''        biome: Option<Preview3dBiomeSample>,\n    block_models: Option<&BlockModelRepository>,"
new = "    '''    biome: Option<Preview3dBiomeSample>,\n    block_models: Option<&BlockModelRepository>,"
count = source.count(old)
if count != 1:
    raise RuntimeError(f"collector signature bootstrap: expected 1 match, got {count}")
path.write_text(source.replace(old, new, 1), encoding="utf-8")

#!/usr/bin/env python3
"""Generate stable map entity icons from Bedrock vanilla and education packs.

The generated PNG files are source assets. Run this script after updating the
bundled Minecraft version, then rebuild BMCBL so build.rs embeds the results.
"""

from __future__ import annotations

import sys
from pathlib import Path

# Add scripts directory to sys.path so the entity_icon_generator package can be imported
scripts_dir = Path(__file__).resolve().parent
if str(scripts_dir) not in sys.path:
    sys.path.insert(0, str(scripts_dir))

from entity_icon_generator.main import main

if __name__ == "__main__":
    main()

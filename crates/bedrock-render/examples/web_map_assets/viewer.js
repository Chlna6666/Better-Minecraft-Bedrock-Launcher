(function () {
  let mapData = window.BEDROCK_WEB_MAP;
  if (!mapData) {
    throw new Error("map-data.js is missing or failed to load");
  }

  let tileIndex = window.BEDROCK_WEB_TILE_INDEX || { dimensions: [] };
  let pluginData = window.BEDROCK_WEB_PLUGINS || { plugins: [] };
  let tileSize = mapData.tileSize;
  let currentDimension;
  let currentMode;
  let scale = 1;
  let offsetX = 0;
  let offsetY = 0;
  let dragging = false;
  let dragStart = null;
  let offsetStart = null;
  let contextBlock = null;
  let renderQueued = false;
  let mapDataReloading = false;
  let tileIndexReloading = false;
  let mapDataFailures = 0;
  let tileIndexFailures = 0;
  let markersByDimension = {};
  let overlayOptions = {
    axis: true,
    denseGrid: false,
    binaryCoordinates: false,
    ruler: true,
  };

  const map = document.getElementById("map");
  const gridCanvas = document.getElementById("gridCanvas");
  const layer = document.getElementById("layer");
  const markerLayer = document.getElementById("markerLayer");
  const pluginLayer = document.getElementById("pluginLayer");
  const pluginPanel = document.getElementById("pluginPanel");
  const statusEl = document.getElementById("status");
  const coordHud = document.getElementById("coordHud");
  const contextMenu = document.getElementById("contextMenu");
  const contextCoords = document.getElementById("contextCoords");
  const copyTpButton = document.getElementById("copyTp");
  const addMarkerButton = document.getElementById("addMarker");
  const jumpToCoordinateButton = document.getElementById("jumpToCoordinate");
  const resetViewButton = document.getElementById("resetView");
  const clearMarkersButton = document.getElementById("clearMarkers");
  const dimSelect = document.getElementById("dimension");
  const modeSelect = document.getElementById("mode");
  const zoomLabel = document.getElementById("zoomLabel");

  const viewerConfig = {
    prefetchRadius: 1,
    retainRadius: 2,
    maxImageLoads: 8,
    refreshMs: 2000,
    ...(mapData.viewer || {}),
  };

  class TileManager {
    constructor(element) {
      this.element = element;
      this.entries = new Map();
      this.queue = [];
      this.activeLoads = 0;
      this.maxImageLoads = Math.max(1, Number(viewerConfig.maxImageLoads) || 8);
    }

    update({ visibleTiles, retainTiles, dimensionId, modeId }) {
      const retainKeys = new Set(retainTiles.map(([x, z]) => this.key(dimensionId, modeId, x, z)));
      for (const [key, entry] of this.entries) {
        if (!retainKeys.has(key)) {
          entry.element.remove();
          this.entries.delete(key);
        }
      }
      this.element.style.transform = `translate(${offsetX}px, ${offsetY}px) scale(${scale})`;
      this.element.style.setProperty("--tile-size", `${tileSize}px`);
      for (const [x, z] of visibleTiles) {
        this.ensureTile(dimensionId, modeId, x, z, true);
      }
      for (const [x, z] of retainTiles) {
        this.ensureTile(dimensionId, modeId, x, z, false);
      }
      this.pumpQueue();
    }

    ensureTile(dimensionId, modeId, x, z, highPriority) {
      const key = this.key(dimensionId, modeId, x, z);
      let entry = this.entries.get(key);
      if (!entry) {
        const img = document.createElement("img");
        img.className = "tile loading";
        img.loading = "lazy";
        img.decoding = "async";
        img.draggable = false;
        img.style.left = `${x * tileSize}px`;
        img.style.top = `${z * tileSize}px`;
        img.id = `tile-${key.replaceAll("/", "-")}`;
        entry = {
          key,
          element: img,
          src: mapData.tilePath(dimensionId, modeId, x, z),
          state: "queued",
          attempts: 0,
          retryAt: 0,
        };
        img.onload = () => {
          entry.state = "loaded";
          img.classList.remove("loading", "failed");
          this.activeLoads = Math.max(0, this.activeLoads - 1);
          this.pumpQueue();
        };
        img.onerror = () => {
          entry.state = "failed";
          entry.attempts += 1;
          entry.retryAt = Date.now() + Math.min(30000, 1000 * 2 ** entry.attempts);
          img.classList.remove("loading");
          img.classList.add("failed");
          this.activeLoads = Math.max(0, this.activeLoads - 1);
          this.pumpQueue();
        };
        this.entries.set(key, entry);
        this.element.appendChild(img);
      }
      entry.element.style.left = `${x * tileSize}px`;
      entry.element.style.top = `${z * tileSize}px`;
      if (entry.state === "queued" || (entry.state === "failed" && Date.now() >= entry.retryAt)) {
        this.enqueue(entry, highPriority);
      }
    }

    enqueue(entry, highPriority) {
      if (this.queue.includes(entry)) return;
      if (highPriority) {
        this.queue.unshift(entry);
      } else {
        this.queue.push(entry);
      }
    }

    pumpQueue() {
      while (this.activeLoads < this.maxImageLoads && this.queue.length > 0) {
        const entry = this.queue.shift();
        if (!this.entries.has(entry.key) || entry.state === "loading" || entry.state === "loaded") {
          continue;
        }
        if (entry.state === "failed" && Date.now() < entry.retryAt) {
          continue;
        }
        entry.state = "loading";
        entry.element.classList.add("loading");
        entry.element.classList.remove("failed");
        this.activeLoads += 1;
        const separator = entry.src.includes("?") ? "&" : "?";
        entry.element.src = entry.attempts > 0 ? `${entry.src}${separator}retry=${entry.attempts}` : entry.src;
      }
    }

    key(dimensionId, modeId, x, z) {
      return mapData.tileId(dimensionId, modeId, x, z);
    }
  }

  const tileManager = new TileManager(layer);

  currentDimension = mapData.dimensions.find(d => d.bounds) || mapData.dimensions[0];
  fillDimensions();
  fillModes();
  centerOnBlock(0, 0);
  scheduleRender();
  scheduleReloads();

  function fillDimensions() {
    const selectedId = currentDimension && currentDimension.id;
    dimSelect.innerHTML = "";
    for (const dim of mapData.dimensions) {
      const option = document.createElement("option");
      option.value = dim.id;
      option.textContent = dim.label + (dim.bounds ? "" : " (empty)");
      dimSelect.appendChild(option);
    }
    if (selectedId) dimSelect.value = selectedId;
  }

  function fillModes(preferredModeId) {
    modeSelect.innerHTML = "";
    for (const mode of currentDimension.modes) {
      const option = document.createElement("option");
      option.value = mode.id;
      option.textContent = `${mode.label} (${mode.tiles} tiles)`;
      modeSelect.appendChild(option);
    }
    currentMode =
      currentDimension.modes.find(mode => mode.id === preferredModeId) ||
      currentDimension.modes.find(mode => currentMode && mode.id === currentMode.id) ||
      currentDimension.modes[0] ||
      null;
    if (currentMode) modeSelect.value = currentMode.id;
  }

  function indexedTilesForDimension(dimensionId) {
    const indexed = tileIndex.dimensions.find(d => d.id === dimensionId);
    return indexed && Array.isArray(indexed.tiles) ? indexed.tiles : [];
  }

  function indexedTileBounds(dimensionId) {
    const tiles = indexedTilesForDimension(dimensionId);
    if (!tiles.length) return null;
    let minX = Infinity;
    let maxX = -Infinity;
    let minZ = Infinity;
    let maxZ = -Infinity;
    for (const [x, z] of tiles) {
      minX = Math.min(minX, x);
      maxX = Math.max(maxX, x);
      minZ = Math.min(minZ, z);
      maxZ = Math.max(maxZ, z);
    }
    return { minX, maxX, minZ, maxZ };
  }

  function blockToMapPixel(blockX, blockZ) {
    const pixelsPerBlock = mapData.pixelsPerBlock || 1;
    return {
      x: blockX * pixelsPerBlock / mapData.blocksPerPixel,
      z: blockZ * pixelsPerBlock / mapData.blocksPerPixel,
    };
  }

  function mapPixelToBlock(mapPixelX, mapPixelZ) {
    const pixelsPerBlock = mapData.pixelsPerBlock || 1;
    return {
      x: Math.floor(mapPixelX * mapData.blocksPerPixel / pixelsPerBlock),
      z: Math.floor(mapPixelZ * mapData.blocksPerPixel / pixelsPerBlock),
    };
  }

  function screenToBlock(clientX, clientY) {
    const rect = map.getBoundingClientRect();
    const mapPixelX = (clientX - rect.left - offsetX) / scale;
    const mapPixelZ = (clientY - rect.top - offsetY) / scale;
    return mapPixelToBlock(mapPixelX, mapPixelZ);
  }

  function centerOnBlock(blockX, blockZ) {
    const point = blockToMapPixel(blockX, blockZ);
    offsetX = map.clientWidth / 2 - point.x * scale;
    offsetY = map.clientHeight / 2 - point.z * scale;
  }

  function tpCommand(block) {
    return `/tp ${block.x} ~ ${block.z}`;
  }

  function coordinateText(block) {
    if (!overlayOptions.binaryCoordinates) {
      return `X ${block.x} · Z ${block.z}`;
    }
    const xBinary = block.x < 0 ? `-${Math.abs(block.x).toString(2)}` : block.x.toString(2);
    const zBinary = block.z < 0 ? `-${Math.abs(block.z).toString(2)}` : block.z.toString(2);
    return `X ${block.x} (${xBinary}₂) · Z ${block.z} (${zBinary}₂)`;
  }

  function markersForCurrentDimension() {
    if (!currentDimension) return [];
    return markersByDimension[currentDimension.id] || [];
  }

  function scheduleRender() {
    if (renderQueued) return;
    renderQueued = true;
    window.requestAnimationFrame(() => {
      renderQueued = false;
      render();
    });
  }

  function render() {
    markerLayer.innerHTML = "";
    drawGrid();
    renderMarkers();
    renderPlugins();
    if (!currentDimension || !currentMode || !currentDimension.bounds) {
      statusEl.textContent = "No loaded chunks for this dimension.";
      return;
    }
    if (!currentMode.rendered) {
      statusEl.textContent = `${currentDimension.label} / ${currentMode.label} is rendering...`;
      return;
    }
    const tileBounds = indexedTileBounds(currentDimension.id) || mapData.tileBounds(currentDimension.bounds);
    if (!tileBounds) {
      statusEl.textContent = "No loaded chunks for this dimension.";
      return;
    }
    const visibleBounds = viewportTileBounds(tileBounds, viewerConfig.prefetchRadius);
    const retainBounds = viewportTileBounds(tileBounds, viewerConfig.retainRadius);
    const visibleTiles = visibleTilesInBounds(visibleBounds);
    const retainTiles = visibleTilesInBounds(retainBounds);
    tileManager.update({
      visibleTiles,
      retainTiles,
      dimensionId: currentDimension.id,
      modeId: currentMode.id,
    });
    markerLayer.style.transform = `translate(${offsetX}px, ${offsetY}px) scale(${scale})`;
    pluginLayer.style.transform = `translate(${offsetX}px, ${offsetY}px) scale(${scale})`;
    zoomLabel.textContent = `${Math.round(scale * 100)}%`;
    const centerBlock = screenToBlock(map.clientWidth / 2, map.clientHeight / 2);
    statusEl.textContent = `${currentDimension.label} / ${currentMode.label} · visible tiles ${visibleTiles.length} · center ${coordinateText(centerBlock)}`;
  }

  function viewportTileBounds(tileBounds, radius) {
    const padding = Math.max(0, Number(radius) || 0);
    return {
      minX: Math.max(tileBounds.minX, Math.floor((-offsetX) / (tileSize * scale)) - padding),
      maxX: Math.min(tileBounds.maxX, Math.floor((map.clientWidth - offsetX) / (tileSize * scale)) + padding),
      minZ: Math.max(tileBounds.minZ, Math.floor((-offsetY) / (tileSize * scale)) - padding),
      maxZ: Math.min(tileBounds.maxZ, Math.floor((map.clientHeight - offsetY) / (tileSize * scale)) + padding),
    };
  }

  function visibleTilesInBounds(bounds) {
    const indexedTiles = indexedTilesForDimension(currentDimension.id);
    if (indexedTiles.length > 0) {
      return indexedTiles.filter(([x, z]) =>
        x >= bounds.minX && x <= bounds.maxX && z >= bounds.minZ && z <= bounds.maxZ
      );
    }
    const tiles = [];
    for (let z = bounds.minZ; z <= bounds.maxZ; z++) {
      for (let x = bounds.minX; x <= bounds.maxX; x++) {
        tiles.push([x, z]);
      }
    }
    return tiles;
  }

  function blockToScreen(blockX, blockZ) {
    const point = blockToMapPixel(blockX, blockZ);
    return {
      x: offsetX + point.x * scale,
      y: offsetY + point.z * scale,
    };
  }

  function viewportBlockBounds() {
    const topLeft = screenToBlock(0, 0);
    const bottomRight = screenToBlock(map.clientWidth, map.clientHeight);
    return {
      minX: Math.min(topLeft.x, bottomRight.x),
      maxX: Math.max(topLeft.x, bottomRight.x),
      minZ: Math.min(topLeft.z, bottomRight.z),
      maxZ: Math.max(topLeft.z, bottomRight.z),
    };
  }

  function adjustedGridStep(baseStep, minValue, maxValue, maxLines) {
    let step = Math.max(1, baseStep);
    while ((maxValue - minValue) / step > maxLines) step *= 2;
    return step;
  }

  function formattedCoordinateValue(value) {
    if (!overlayOptions.binaryCoordinates) return String(value);
    return value < 0 ? `-${Math.abs(value).toString(2)}₂` : `${value.toString(2)}₂`;
  }

  function drawGridLines(ctx, bounds, step, color, width, labels) {
    const minX = Math.floor(bounds.minX / step) * step;
    const maxX = Math.ceil(bounds.maxX / step) * step;
    const minZ = Math.floor(bounds.minZ / step) * step;
    const maxZ = Math.ceil(bounds.maxZ / step) * step;
    ctx.strokeStyle = color;
    ctx.lineWidth = width;
    ctx.beginPath();
    for (let x = minX; x <= maxX; x += step) {
      const screen = blockToScreen(x, 0);
      ctx.moveTo(Math.round(screen.x) + 0.5, 0);
      ctx.lineTo(Math.round(screen.x) + 0.5, map.clientHeight);
    }
    for (let z = minZ; z <= maxZ; z += step) {
      const screen = blockToScreen(0, z);
      ctx.moveTo(0, Math.round(screen.y) + 0.5);
      ctx.lineTo(map.clientWidth, Math.round(screen.y) + 0.5);
    }
    ctx.stroke();
    if (!labels) return;
    ctx.fillStyle = "rgba(232,237,242,.82)";
    ctx.font = "11px system-ui, Segoe UI, sans-serif";
    for (let x = minX; x <= maxX; x += step) {
      const screen = blockToScreen(x, 0);
      if (screen.x >= 4 && screen.x <= map.clientWidth - 36) {
        ctx.fillText(formattedCoordinateValue(x), screen.x + 4, 22);
      }
    }
    for (let z = minZ; z <= maxZ; z += step) {
      const screen = blockToScreen(0, z);
      if (screen.y >= 20 && screen.y <= map.clientHeight - 8) {
        ctx.fillText(formattedCoordinateValue(z), 8, screen.y - 4);
      }
    }
  }

  function drawAxes(ctx) {
    if (!overlayOptions.axis) return;
    const originX = blockToScreen(0, 0).x;
    const originY = blockToScreen(0, 0).y;
    ctx.lineWidth = 2;
    if (originX >= 0 && originX <= map.clientWidth) {
      ctx.strokeStyle = "rgba(255,86,86,.86)";
      ctx.beginPath();
      ctx.moveTo(Math.round(originX) + 0.5, 0);
      ctx.lineTo(Math.round(originX) + 0.5, map.clientHeight);
      ctx.stroke();
    }
    if (originY >= 0 && originY <= map.clientHeight) {
      ctx.strokeStyle = "rgba(89,165,255,.86)";
      ctx.beginPath();
      ctx.moveTo(0, Math.round(originY) + 0.5);
      ctx.lineTo(map.clientWidth, Math.round(originY) + 0.5);
      ctx.stroke();
    }
  }

  function drawRuler(ctx) {
    if (!overlayOptions.ruler) return;
    const candidates = [16, 32, 64, 128, 256, 512, 1024, 2048, 4096];
    let rulerBlocks = candidates[0];
    const pixelsPerBlock = mapData.pixelsPerBlock || 1;
    for (const candidate of candidates) {
      const pixels = candidate * pixelsPerBlock * scale / mapData.blocksPerPixel;
      if (pixels >= 90 && pixels <= 190) {
        rulerBlocks = candidate;
        break;
      }
      if (pixels < 90) rulerBlocks = candidate;
    }
    const rulerPixels = rulerBlocks * pixelsPerBlock * scale / mapData.blocksPerPixel;
    const x = map.clientWidth - rulerPixels - 24;
    const y = map.clientHeight - 36;
    ctx.strokeStyle = "rgba(255,255,255,.88)";
    ctx.fillStyle = "rgba(16,18,20,.72)";
    ctx.lineWidth = 2;
    ctx.fillRect(x - 8, y - 18, rulerPixels + 16, 30);
    ctx.beginPath();
    ctx.moveTo(x, y);
    ctx.lineTo(x + rulerPixels, y);
    ctx.moveTo(x, y - 5);
    ctx.lineTo(x, y + 5);
    ctx.moveTo(x + rulerPixels, y - 5);
    ctx.lineTo(x + rulerPixels, y + 5);
    ctx.stroke();
    ctx.fillStyle = "#fff";
    ctx.font = "12px system-ui, Segoe UI, sans-serif";
    ctx.fillText(`${rulerBlocks} blocks / ${rulerBlocks / 16} chunks`, x, y - 8);
  }

  function drawGrid() {
    const width = map.clientWidth;
    const height = map.clientHeight;
    const devicePixelRatio = window.devicePixelRatio || 1;
    gridCanvas.width = Math.max(1, Math.floor(width * devicePixelRatio));
    gridCanvas.height = Math.max(1, Math.floor(height * devicePixelRatio));
    gridCanvas.style.width = `${width}px`;
    gridCanvas.style.height = `${height}px`;
    const ctx = gridCanvas.getContext("2d");
    ctx.setTransform(devicePixelRatio, 0, 0, devicePixelRatio, 0, 0);
    ctx.clearRect(0, 0, width, height);
    const bounds = viewportBlockBounds();
    const tileBlocks = mapData.chunksPerTile * 16;
    const tileStep = adjustedGridStep(tileBlocks, bounds.minX, bounds.maxX, 140);
    drawGridLines(ctx, bounds, tileStep, "rgba(255,255,255,.22)", 1.4, true);
    const chunkStep = adjustedGridStep(16, bounds.minX, bounds.maxX, 280);
    const pixelsPerBlock = mapData.pixelsPerBlock || 1;
    if (overlayOptions.denseGrid || (chunkStep * pixelsPerBlock * scale / mapData.blocksPerPixel) >= 18) {
      drawGridLines(ctx, bounds, chunkStep, "rgba(120,190,255,.20)", 1, overlayOptions.denseGrid);
    }
    if (overlayOptions.denseGrid) {
      const scaledPixelsPerBlock = pixelsPerBlock * scale / mapData.blocksPerPixel;
      const baseFineStep = scaledPixelsPerBlock >= 4 ? 1 : scaledPixelsPerBlock >= 2 ? 2 : scaledPixelsPerBlock >= 1 ? 4 : 8;
      const fineStep = adjustedGridStep(baseFineStep, bounds.minX, bounds.maxX, 360);
      if (fineStep < 16) {
        drawGridLines(ctx, bounds, fineStep, "rgba(255,255,255,.08)", 1, false);
      }
    }
    drawAxes(ctx);
    drawRuler(ctx);
  }

  function renderMarkers() {
    markerLayer.style.transform = `translate(${offsetX}px, ${offsetY}px) scale(${scale})`;
    for (const marker of markersForCurrentDimension()) {
      const point = blockToMapPixel(marker.x, marker.z);
      const element = document.createElement("div");
      element.className = "marker";
      element.dataset.label = marker.label;
      element.style.left = `${point.x}px`;
      element.style.top = `${point.z}px`;
      markerLayer.appendChild(element);
    }
  }

  function renderPlugins() {
    pluginLayer.style.transform = `translate(${offsetX}px, ${offsetY}px) scale(${scale})`;
    if (!window.BedrockWebPluginRuntime || !currentDimension) return;
    window.BedrockWebPluginRuntime.render({
      plugins: pluginData,
      dimensionId: currentDimension.id,
      layer: pluginLayer,
      panel: pluginPanel,
      helpers: { blockToMapPixel },
    });
  }

  function scheduleReloads() {
    window.setTimeout(reloadMapData, reloadDelay(mapDataFailures));
    window.setTimeout(reloadTileIndex, reloadDelay(tileIndexFailures));
  }

  function reloadDelay(failures) {
    const base = Math.max(500, Number(viewerConfig.refreshMs) || 2000);
    return Math.min(60000, base * 2 ** Math.min(5, failures));
  }

  function needsRefresh() {
    return mapData.dimensions.some(dimension =>
      Array.isArray(dimension.modes) && dimension.modes.some(mode => !mode.rendered)
    );
  }

  function reloadMapData() {
    if (mapDataReloading || !needsRefresh()) return;
    mapDataReloading = true;
    const previousDimensionId = currentDimension && currentDimension.id;
    const previousModeId = currentMode && currentMode.id;
    const script = document.createElement("script");
    script.src = `map-data.js?ts=${Date.now()}`;
    script.onload = () => {
      script.remove();
      mapDataReloading = false;
      mapDataFailures = 0;
      if (!window.BEDROCK_WEB_MAP) return;
      mapData = window.BEDROCK_WEB_MAP;
      tileSize = mapData.tileSize;
      Object.assign(viewerConfig, mapData.viewer || {});
      currentDimension =
        mapData.dimensions.find(d => d.id === previousDimensionId) ||
        mapData.dimensions.find(d => d.bounds) ||
        mapData.dimensions[0];
      if (!currentDimension) return;
      fillDimensions();
      fillModes(previousModeId);
      scheduleRender();
      if (needsRefresh()) window.setTimeout(reloadMapData, reloadDelay(mapDataFailures));
    };
    script.onerror = () => {
      script.remove();
      mapDataReloading = false;
      mapDataFailures += 1;
      window.setTimeout(reloadMapData, reloadDelay(mapDataFailures));
    };
    document.head.appendChild(script);
  }

  function reloadTileIndex() {
    if (tileIndexReloading || !needsRefresh()) return;
    tileIndexReloading = true;
    const script = document.createElement("script");
    script.src = `tile-index.js?ts=${Date.now()}`;
    script.onload = () => {
      script.remove();
      tileIndexReloading = false;
      tileIndexFailures = 0;
      if (window.BEDROCK_WEB_TILE_INDEX) {
        tileIndex = window.BEDROCK_WEB_TILE_INDEX;
        scheduleRender();
      }
      if (needsRefresh()) window.setTimeout(reloadTileIndex, reloadDelay(tileIndexFailures));
    };
    script.onerror = () => {
      script.remove();
      tileIndexReloading = false;
      tileIndexFailures += 1;
      window.setTimeout(reloadTileIndex, reloadDelay(tileIndexFailures));
    };
    document.head.appendChild(script);
  }

  function closeContextMenu() {
    contextMenu.classList.remove("open");
  }

  function positionContextMenu(clientX, clientY) {
    const margin = 8;
    const rect = contextMenu.getBoundingClientRect();
    const left = Math.min(clientX, window.innerWidth - rect.width - margin);
    const top = Math.min(clientY, window.innerHeight - rect.height - margin);
    contextMenu.style.left = `${Math.max(margin, left)}px`;
    contextMenu.style.top = `${Math.max(margin, top)}px`;
  }

  function openContextMenu(event) {
    event.preventDefault();
    contextBlock = screenToBlock(event.clientX, event.clientY);
    contextCoords.textContent = coordinateText(contextBlock);
    copyTpButton.textContent = `复制 ${tpCommand(contextBlock)}`;
    contextMenu.classList.add("open");
    positionContextMenu(event.clientX, event.clientY);
  }

  async function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return;
    }
    const textarea = document.createElement("textarea");
    textarea.value = text;
    textarea.style.position = "fixed";
    textarea.style.left = "-9999px";
    document.body.appendChild(textarea);
    textarea.focus();
    textarea.select();
    document.execCommand("copy");
    textarea.remove();
  }

  function addMarkerAt(block) {
    if (!currentDimension) return;
    const markers = markersByDimension[currentDimension.id] || [];
    markers.push({
      x: block.x,
      z: block.z,
      label: `${block.x}, ${block.z}`,
    });
    markersByDimension[currentDimension.id] = markers;
    scheduleRender();
  }

  function parseCoordinateInput(value) {
    const parts = value.trim().split(/[,\s]+/).filter(Boolean);
    if (parts.length < 2) return null;
    const x = Number.parseInt(parts[0], 10);
    const z = Number.parseInt(parts[1], 10);
    if (!Number.isFinite(x) || !Number.isFinite(z)) return null;
    return { x, z };
  }

  function updateToggleButtons() {
    document.querySelectorAll(".menu-toggle").forEach(button => {
      const key = button.dataset.overlay;
      button.classList.toggle("active", Boolean(overlayOptions[key]));
    });
  }

  dimSelect.addEventListener("change", () => {
    currentDimension = mapData.dimensions.find(d => d.id === dimSelect.value);
    fillModes();
    centerOnBlock(0, 0);
    closeContextMenu();
    scheduleRender();
  });
  modeSelect.addEventListener("change", () => {
    currentMode = currentDimension.modes.find(m => m.id === modeSelect.value);
    closeContextMenu();
    scheduleRender();
  });
  document.getElementById("zoomIn").onclick = () => zoomAt(map.clientWidth / 2, map.clientHeight / 2, 1.25);
  document.getElementById("zoomOut").onclick = () => zoomAt(map.clientWidth / 2, map.clientHeight / 2, 0.8);
  map.addEventListener("wheel", event => {
    event.preventDefault();
    zoomAt(event.clientX, event.clientY, event.deltaY < 0 ? 1.15 : 0.87);
  }, { passive: false });
  map.addEventListener("contextmenu", openContextMenu);
  map.addEventListener("mousedown", event => {
    if (event.button !== 0) return;
    closeContextMenu();
    dragging = true;
    map.classList.add("dragging");
    dragStart = { x: event.clientX, y: event.clientY };
    offsetStart = { x: offsetX, y: offsetY };
  });
  window.addEventListener("mouseup", () => {
    dragging = false;
    map.classList.remove("dragging");
  });
  window.addEventListener("mousemove", event => {
    const block = screenToBlock(event.clientX, event.clientY);
    coordHud.textContent = coordinateText(block);
    if (!dragging) return;
    offsetX = offsetStart.x + event.clientX - dragStart.x;
    offsetY = offsetStart.y + event.clientY - dragStart.y;
    scheduleRender();
  });
  window.addEventListener("resize", scheduleRender);
  window.addEventListener("keydown", event => {
    if (event.key === "Escape") closeContextMenu();
  });
  copyTpButton.addEventListener("click", async () => {
    if (!contextBlock) return;
    await copyText(tpCommand(contextBlock));
    closeContextMenu();
  });
  addMarkerButton.addEventListener("click", () => {
    if (!contextBlock) return;
    addMarkerAt(contextBlock);
    closeContextMenu();
  });
  jumpToCoordinateButton.addEventListener("click", () => {
    const value = window.prompt("输入坐标，例如: 2233 -2940");
    const coordinate = value ? parseCoordinateInput(value) : null;
    if (!coordinate) return;
    centerOnBlock(coordinate.x, coordinate.z);
    closeContextMenu();
    scheduleRender();
  });
  resetViewButton.addEventListener("click", () => {
    centerOnBlock(0, 0);
    closeContextMenu();
    scheduleRender();
  });
  clearMarkersButton.addEventListener("click", () => {
    if (currentDimension) markersByDimension[currentDimension.id] = [];
    closeContextMenu();
    scheduleRender();
  });
  document.querySelectorAll(".menu-toggle").forEach(button => {
    button.addEventListener("click", () => {
      const key = button.dataset.overlay;
      overlayOptions[key] = !overlayOptions[key];
      updateToggleButtons();
      scheduleRender();
    });
  });
  updateToggleButtons();

  function zoomAt(clientX, clientY, factor) {
    const rect = map.getBoundingClientRect();
    const x = clientX - rect.left;
    const y = clientY - rect.top;
    const worldX = (x - offsetX) / scale;
    const worldY = (y - offsetY) / scale;
    scale = Math.max(0.125, Math.min(8, scale * factor));
    offsetX = x - worldX * scale;
    offsetY = y - worldY * scale;
    scheduleRender();
  }
})();

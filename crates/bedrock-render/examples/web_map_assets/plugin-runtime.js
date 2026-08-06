(function () {
  function asArray(value) {
    return Array.isArray(value) ? value : [];
  }

  function itemEntries(item) {
    if (Array.isArray(item.entries)) return item.entries;
    if (Array.isArray(item.messages)) return item.messages;
    if (Array.isArray(item.points)) return item.points;
    return [item];
  }

  function itemDimensionMatches(item, dimensionId) {
    return item.dimension === dimensionId || item.dimension === "*" || item.dimension === "all";
  }

  function appendPoint(layer, helpers, item, entry) {
    const x = Number(entry.x);
    const z = Number(entry.z);
    if (!Number.isFinite(x) || !Number.isFinite(z)) return;
    const point = helpers.blockToMapPixel(x, z);
    const element = document.createElement("div");
    element.className = "plugin-point";
    element.dataset.label = entry.label || entry.name || item.label || item.id || "point";
    element.style.left = `${point.x}px`;
    element.style.top = `${point.z}px`;
    element.style.setProperty("--plugin-color", entry.color || item.color || "#48d597");
    layer.appendChild(element);
  }

  function appendArea(layer, helpers, item, entry) {
    const bounds = entry.bounds || item.bounds;
    if (!bounds) return;
    const minX = Number(bounds.minX);
    const minZ = Number(bounds.minZ);
    const maxX = Number(bounds.maxX);
    const maxZ = Number(bounds.maxZ);
    if (![minX, minZ, maxX, maxZ].every(Number.isFinite)) return;
    const topLeft = helpers.blockToMapPixel(Math.min(minX, maxX), Math.min(minZ, maxZ));
    const bottomRight = helpers.blockToMapPixel(Math.max(minX, maxX), Math.max(minZ, maxZ));
    const element = document.createElement("div");
    element.className = "plugin-area";
    element.style.left = `${topLeft.x}px`;
    element.style.top = `${topLeft.z}px`;
    element.style.width = `${Math.max(1, bottomRight.x - topLeft.x)}px`;
    element.style.height = `${Math.max(1, bottomRight.z - topLeft.z)}px`;
    element.style.setProperty("--plugin-color", entry.color || item.color || "#ffd166");
    layer.appendChild(element);
  }

  function appendPanel(panel, item) {
    const card = document.createElement("section");
    card.className = "plugin-card";
    const title = document.createElement("div");
    title.className = "plugin-title";
    title.textContent = item.label || item.id || item.type;
    card.appendChild(title);
    const text = document.createElement("div");
    text.textContent = item.text || item.content || "";
    card.appendChild(text);
    panel.appendChild(card);
  }

  function appendChat(panel, item) {
    const card = document.createElement("section");
    card.className = "plugin-card";
    const title = document.createElement("div");
    title.className = "plugin-title";
    title.textContent = item.label || "Chat";
    card.appendChild(title);
    for (const entry of itemEntries(item).slice(-40)) {
      const line = document.createElement("div");
      line.className = "chat-line";
      const player = document.createElement("span");
      player.className = "chat-player";
      player.textContent = entry.player || entry.name || "server";
      line.appendChild(player);
      line.appendChild(document.createTextNode(`: ${entry.message || entry.text || ""}`));
      card.appendChild(line);
    }
    panel.appendChild(card);
  }

  window.BedrockWebPluginRuntime = {
    render({ plugins, dimensionId, layer, panel, helpers }) {
      layer.innerHTML = "";
      panel.innerHTML = "";
      const pluginList = asArray(plugins && plugins.plugins);
      for (const plugin of pluginList) {
        for (const item of asArray(plugin.items)) {
          if (!itemDimensionMatches(item, dimensionId)) continue;
          if (item.type === "markers" || item.type === "points") {
            for (const entry of itemEntries(item)) appendPoint(layer, helpers, item, entry);
          } else if (item.type === "areas") {
            for (const entry of itemEntries(item)) appendArea(layer, helpers, item, entry);
          } else if (item.type === "chat") {
            appendChat(panel, item);
          } else if (item.type === "panel") {
            appendPanel(panel, item);
          }
        }
      }
      panel.classList.toggle("open", panel.childElementCount > 0);
    },
  };
})();

// SPDX-License-Identifier: MIT
// BMCBL standalone Bedrock Script API exporter. No third-party extractor code.
// BDS, not the Script API's aliases, is the final authority on persisted states.
// Namespace import avoids import-time failures when old API versions omit a symbol.
import * as mc from "@minecraft/server";
import { fallbackBlockIds } from "./block_ids.js";
const { BlockPermutation, BlockStates, BlockTypes, system, world } = mc;
const StructureSaveMode = mc.StructureSaveMode;

const EDGE = 32;
const BATCH_SIZE = EDGE * EDGE;
const INTEGER_RANGE = 15;
const MAX_PER_BLOCK = 250000;
const VOID_POSITION = { x: 0, y: 240, z: 0 };

function emit(event, data) {
  console.log("BMCBL-DUMP-" + event + " " + JSON.stringify(data));
}

function* combinations(names, domains, at, current) {
  if (at === names.length) {
    yield { ...current };
    return;
  }
  for (const value of domains[at]) {
    current[names[at]] = value;
    yield* combinations(names, domains, at + 1, current);
  }
  delete current[names[at]];
}

function valuesFor(name, fallback) {
  const definition = BlockStates?.get?.(name);
  const values = Array.from(definition?.validValues ?? []);
  if (values.length > 0 && typeof values[0] === "number") {
    // Numeric value domains are not always complete for a particular block.
    for (let i = 0; i <= INTEGER_RANGE; i++) {
      if (!values.includes(i)) values.push(i);
    }
  }
  if (values.length === 0) values.push(fallback);
  return values;
}

function* candidates(block, stats) {
  let initial;
  try {
    initial = BlockPermutation.resolve(block);
  } catch (error) {
    throw new Error("Cannot resolve default for " + block + ": " + String(error));
  }
  const defaults = typeof initial.getAllStates === "function" ? initial.getAllStates()
    : typeof initial.getAllProperties === "function" ? initial.getAllProperties() : {};
  const keys = Object.keys(defaults).sort();
  const domains = keys.map((key) => valuesFor(key, defaults[key]));
  const attempted = domains.reduce((count, values) => count * values.length, 1);
  if (attempted > MAX_PER_BLOCK) {
    throw new Error("Refusing to silently truncate " + block + ": " + attempted + " candidates");
  }
  for (const states of combinations(keys, domains, 0, {})) {
    stats.attempted++;
    try {
      yield BlockPermutation.resolve(block, states);
    } catch {
      // This is an expected invalid combination, not a failed export.
      stats.rejected++;
      yield null;
    }
  }
}

function saveBatch(kind, serial, entries, stats) {
  const id = "bmcbl:" + kind + serial;
  const structure = world.structureManager.createEmpty(
    id, { x: EDGE, y: 1, z: EDGE }, StructureSaveMode?.Memory
  );
  let placed = 0;
  for (let index = 0; index < entries.length; index++) {
    try {
      structure.setBlockPermutation(
        { x: index % EDGE, y: 0, z: Math.floor(index / EDGE) },
        entries[index]
      );
      placed++;
    } catch (error) {
      // Script API resolution is more permissive than Structure serialization.
      // Keep the actual saved palette as the authority, not accepted aliases.
      stats.structureRejected++;
      if (stats.structureRejected <= 8) {
        emit("SKIP", { kind, block: entries[index].type.id, error: String(error) });
      }
    }
  }
  // Persist without placing blocks into terrain (which recalculates neighbours).
  structure.saveToWorld();
  stats.structures++;
  stats.placed += placed;
  emit("BATCH", { id, kind, placed, rejected: entries.length - placed });
}

function* putStructureVoid() {
  const dimension = world.getDimension("overworld");
  try {
    dimension.runCommand(
      "tickingarea add 0 240 0 0 240 0 bmcbl_catalog true"
    );
  } catch (error) {
    emit("WARNING", { operation: "tickingarea", message: String(error) });
  }
  let block;
  // A single system.runJob yield does not necessarily mean one game tick.
  // Wait in game-time (up to 60 seconds), not a small number of scheduler yields.
  const deadline = system.currentTick + 1200;
  while (system.currentTick < deadline) {
    try {
      block = dimension.getBlock(VOID_POSITION);
      if (block) break;
    } catch { /* the chunk is not ready yet */ }
    yield;
  }
  if (!block) throw new Error("Structure void chunk could not be loaded");
  block.setPermutation(BlockPermutation.resolve("minecraft:structure_void"));
}

function* collect() {
  const ids = BlockTypes.getAll().map((entry) => entry.id).sort();
  const stats = {
    blocks: ids.length, attempted: 0, rejected: 0,
    structures: 0, placed: 0, permutationStructures: 0,
    defaultStructures: 0, structureRejected: 0
  };
  emit("BEGIN", { blocks: ids.length });
  let serial = 0;
  let batch = [];
  let attemptedAtYield = 0;
  for (const block of ids) {
    for (const candidate of candidates(block, stats)) {
      if (candidate) batch.push(candidate);
      if (batch.length === BATCH_SIZE) {
        saveBatch("p", ++serial, batch, stats);
        stats.permutationStructures++;
        batch = [];
        yield;
      }
      if (stats.attempted - attemptedAtYield >= 256) {
        attemptedAtYield = stats.attempted;
        yield;
      }
    }
    yield;
  }
  if (batch.length) {
    saveBatch("p", ++serial, batch, stats);
    stats.permutationStructures++;
    yield;
  }

  serial = 0;
  batch = [];
  for (const block of ids) {
    batch.push(BlockPermutation.resolve(block));
    if (batch.length === BATCH_SIZE) {
      saveBatch("d", ++serial, batch, stats);
      stats.defaultStructures++;
      batch = [];
      yield;
    }
  }
  if (batch.length) {
    saveBatch("d", ++serial, batch, stats);
    stats.defaultStructures++;
    yield;
  }
  yield* putStructureVoid();
  emit("END", { ...stats, mode: "script_api_structure" });
}

// Earlier stable @minecraft/server releases can expose block type information
// before they support structure serialization. Preserve those observations but
// never assign unknown persisted NBT version/type or numeric IDs.
function* probe(ids, capabilities) {
  emit("BEGIN", { blocks: ids.length, mode: "script_api_probe", capabilities });
  let resolved = 0;
  for (const id of ids) {
    let states = null;
    try {
      const permutation = BlockPermutation?.resolve?.(id);
      if (permutation) resolved++;
      if (typeof permutation?.getAllStates === "function") {
        states = permutation.getAllStates();
      } else if (typeof permutation?.getAllProperties === "function") {
        states = permutation.getAllProperties();
      }
    } catch (error) {
      emit("WARNING", { block: id, message: String(error) });
    }
    emit("PROBE", { name: id, default_states: states });
    yield;
  }
  if (resolved === 0) throw new Error("No candidate block could be resolved from this API");
  emit("END", { mode: "script_api_probe", blocks: ids.length, resolved, placed: 0, structures: 0 });
}

function* chooseCapture() {
  const capabilities = {
    BlockTypes: typeof BlockTypes?.getAll === "function",
    BlockStates: typeof BlockStates?.get === "function",
    BlockPermutation: typeof BlockPermutation?.resolve === "function",
    createEmpty: typeof world?.structureManager?.createEmpty === "function",
    runJob: typeof system?.runJob === "function",
  };
  let ids;
  let registrySource = "script_api";
  if (capabilities.BlockTypes) {
    ids = BlockTypes.getAll().map((block) => block.id).sort();
  } else {
    ids = fallbackBlockIds;
    registrySource = "bds_resource_hints";
    emit("WARNING", {
      capability: "BlockTypes.getAll",
      message: "Unavailable: BDS resource names are only incomplete hints"
    });
  }
  emit("CAPABILITIES", { ...capabilities, registrySource });
  if (!ids || ids.length === 0) {
    throw new Error("No block IDs from Script API or BDS resource hints");
  }
  if (!capabilities.BlockPermutation) {
    throw new Error("BlockPermutation.resolve unavailable: no Script API sampling possible");
  }
  let structuresReady = false;
  if (capabilities.createEmpty && registrySource === "script_api") {
    // Old BDS builds reject Structure creation during early startup/WorldLoad
    // with EngineError. Retry across several game ticks before giving up, so a
    // timing failure is not mistaken for an unsupported API.
    const deadline = system.currentTick + 100;
    let attempt = 0;
    let lastError = null;
    while (!structuresReady && system.currentTick < deadline) {
      try {
        const test = world.structureManager.createEmpty(
          "bmcbl:capability_test_" + attempt, { x: 1, y: 1, z: 1 }, StructureSaveMode?.Memory
        );
        structuresReady = typeof test.setBlockPermutation === "function"
          && typeof test.saveToWorld === "function";
      } catch (error) {
        lastError = String(error);
      }
      attempt++;
      if (!structuresReady) yield;
    }
    if (!structuresReady && lastError) {
      emit("WARNING", { capability: "structure_write", attempts: attempt, message: lastError });
    }
  }
  if (structuresReady && capabilities.BlockStates) {
    if (ids.length < 1000) {
      throw new Error("Script API registry too small for a trusted full capture");
    }
    yield* collect();
  } else {
    yield* probe(ids, { ...capabilities, registrySource });
  }
}

function start() {
  const iterator = (function* () {
    try {
      yield* chooseCapture();
    } catch (error) {
      emit("FAIL", { message: String(error), stack: String(error?.stack ?? "") });
      throw error;
    }
  })();
  if (typeof system.runJob === "function") {
    system.runJob(iterator);
  } else if (typeof system.runInterval === "function") {
    // Legacy scheduler: advance a bounded number of iterations every tick.
    const subscription = system.runInterval(() => {
      try {
        for (let i = 0; i < 16; i++) {
          if (iterator.next().done) {
            system.clearRun(subscription);
            break;
          }
        }
      } catch (error) {
        emit("FAIL", { message: String(error) });
        system.clearRun(subscription);
      }
    }, 1);
  } else {
    emit("FAIL", { message: "No supported script scheduler" });
  }
}

// 1.21.x Script API v1 uses worldInitialize. Script API v2 uses worldLoad.
// Neither event name should be required just to import this module.
const ready = world.afterEvents.worldLoad ?? world.afterEvents.worldInitialize;
if (ready) {
  ready.subscribe(start);
} else {
  // Explicitly documented fallback for versions lacking both after-events.
  system.run(start);
}

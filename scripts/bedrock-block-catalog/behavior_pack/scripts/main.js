// SPDX-License-Identifier: MIT
// BMCBL standalone Bedrock Script API exporter. No third-party extractor code.
// BDS, not the Script API's aliases, is the final authority on persisted states.
import { BlockPermutation, BlockStates, BlockTypes, StructureSaveMode, system, world } from "@minecraft/server";

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
  const definition = BlockStates.get(name);
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
  const defaults = initial.getAllStates();
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
    id, { x: EDGE, y: 1, z: EDGE }, StructureSaveMode.Memory
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
  for (let i = 0; i < 200; i++) {
    try {
      block = dimension.getBlock(VOID_POSITION);
      if (block) break;
    } catch { /* wait until loaded */ }
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
  emit("END", stats);
}

world.afterEvents.worldLoad.subscribe(() => {
  system.runJob((function* () {
    try {
      yield* collect();
    } catch (error) {
      emit("FAIL", { message: String(error), stack: String(error?.stack ?? "") });
      throw error;
    }
  })());
});

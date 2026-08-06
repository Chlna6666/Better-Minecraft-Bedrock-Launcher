# Fixtures

Large real-world fixtures are local-only and intentionally ignored by Git.

To run the optional integration test and large-world benchmarks, copy a Bedrock
world folder here:

```text
tests/fixtures/sample-bedrock-world
```

The copied folder must contain `level.dat` and `db/CURRENT`.

Do not commit real worlds, `.mcworld` exports, or LevelDB table directories.
They are large and can contain player data.

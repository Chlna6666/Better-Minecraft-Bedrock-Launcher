# BlockState upgrade schema fixtures

These fixtures are byte-identical copies from `pmmp/BedrockBlockUpgradeSchema` 5.2.0, commit
`5d7889c9a1cdf9e3cd814d2a104ad69b75116ec7`, licensed CC0-1.0 by the upstream project.

They are intentionally small regression fixtures, not a claim that the complete authoritative corpus
is stored in this test directory. Production corpus synchronization is tracked separately from the
migration executor so the algorithm can be tested without duplicating hundreds of kilobytes in every
test target.

Locked upstream blobs:

- `0011_1.10.0_to_1.12.0.json`: `652e06bda2cec6cff47615eff302dc383ab23602`
- `0121_1.18.10_to_1.18.20.27_beta.json`: `025916a87eb24b86cab30eff279067ea360564b4`
- `0131_1.18.20.27_beta_to_1.18.30.json`: `ccb04ca8e00aa62a7bc37f18b4daf3c2c42d9168`

The 0121/0131 pair is retained specifically because both produce storage version `1.18.10.1` and
therefore exercises Mojang's historical no-version-bump compatibility case.

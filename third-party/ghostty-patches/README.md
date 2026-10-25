# Ghostty native patches

Each `.patch` file here is a reviewed fix to the pinned Ghostty source, applied
in the order `scripts/ghostty-source.json` lists them. The manifest records
each file's SHA-256; `mise run ghostty:prepare` rejects a patch whose bytes
differ. The build applies the patches to its private copy of the source and
never changes `.native/ghostty/source`.

This directory does not ship with Huterm. See
[native patches](../../docs/agents/terminal-engines.md#native-patches) for
verification, the Git requirement, and how to add, rebase, or drop a patch.

Record each patch below: the defect it fixes, evidence, and the condition for
removing it.

# Sparkle provenance

Huterm packages Sparkle 2.9.6 from the official binary release. The archive
URL, release date, SHA-256 digest, extracted tree digest, framework identity,
minimum macOS version, and upstream license digest are pinned in
`scripts/sparkle-source.json`.

`mise run sparkle:prepare` verifies those values before any macOS build uses the
framework. The package copies the complete upstream `LICENSE` file from the
verified distribution into `Huterm.app/Contents/Resources/Sparkle-LICENSE`.

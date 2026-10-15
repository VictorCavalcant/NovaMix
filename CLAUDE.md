# NovaMix

Rust workspace (egui desktop app + CLI) for a Voicemeeter-like virtual audio mixer. See README.md and docs/.

## Rule: always rebuild the .exe

After any change to the program (code in `app/` or `crates/`, Cargo manifests, `build.rs`, docs or
packaging scripts), run the `rebuild-exe` skill before reporting the work as done, so
`dist\NovaMix\novamix.exe` always matches the source.

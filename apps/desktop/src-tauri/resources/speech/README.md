# Voice model

Release bundles carry the voice model and voices listed under `SPEECH` in
`apps/desktop/scripts/dictation-models.mjs`: Gradium's Phonon 31M (English, Apache 2.0), its
tokenizer, and the English voices of Gradium's catalogue. `pnpm bundle` fetches any that are
missing and verifies each against its pinned SHA-256 before Tauri packages this folder. Run
`node scripts/dictation-models.mjs` (from `apps/desktop`) to fetch them for a debug build.

`voices.json` is the catalogue the app reads: who each voice is, in the catalogue's own words. It
is committed; the model, the tokenizer and the voice files are not. A build without them says
the voice model is missing and shows no read-aloud button. The Apache 2.0 text the model and the
voices are released under ships in the app's third-party notices (`third-party/models.json`).

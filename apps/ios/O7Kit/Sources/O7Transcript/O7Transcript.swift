// O7Transcript: the desktop's chat transcript reducer and the helpers its chat screens use, ported
// from TypeScript file by file so a turn reads the same on the phone as on the computer.
//
//   transcript.ts      Transcript.swift (types), Reducer.swift, Render.swift
//   handoff.ts         Handoff.swift
//   memoryContext.ts   MemoryContext.swift
//   tasks.ts           Tasks.swift
//   lib/format.ts      Format.swift (formatCount, formatCost, formatDuration)
//   working.ts         Working.swift
//   toolWords.ts       ToolWords.swift
//   toolDetails.ts     ToolDetails.swift, URLHost.swift (`new URL(...).hostname`)
//   workFold.ts        WorkFold.swift
//   present.ts         Present.swift
//   questions.ts       Questions.swift
//   forms.ts           Forms.swift, JSDate.swift (`new Date(...)`)
//   queueWait.ts       QueueWait.swift
//   lib/diff.ts        Diff.swift
//
// JavaScript's semantics live in JSString.swift, JSNumber.swift and JSValue.swift. The goldens in
// Tests/O7TranscriptTests/Goldens come from the real TypeScript
// (`node --import ./apps/ios/scripts/register-ts.mjs apps/ios/scripts/write-goldens.mts`), and
// GoldenTests compares this port with them.
//
// Known divergences from the TypeScript:
//   - An event type this copy does not know is skipped; the desktop's reducer throws.
//   - Wire enums read values they do not know as their fallback (`LenientEnum`).
//   - JSON objects arrive without their key order (Foundation's decoder does not keep it), so a
//     tool's input lists its fields in code unit order of their names.
//   - Swift strings cannot hold lone surrogates: slicing that would cut a surrogate pair gives
//     U+FFFD for the half, and an escaped lone surrogate in a wire string fails to decode.
//   - Map keys made of ids are compared code unit by code unit, as JavaScript does; dictionaries
//     the app passes in (question drafts, live task ids) use Swift's equality.
//   - `new Date(text)` reads only V8's ISO format (see JSDate.swift); `new URL` reads non-ASCII
//     domains without UTS 46's full mapping table (see URLHost.swift).

# Epic #4: Four-camera live monitoring UI

Depends on epic #1 preview prototype and epic #2 camera state; integrates with epic #3 recording without controlling its lifetime. Implementation has not started.

- [ ] #17: Four selectable preview slots, 2x2 layout, and camera swapping across 1–16 cameras.
- [ ] #18: Touch-friendly camera browser, honest recording/reconnect status, aliases, and persistent selections.
- [ ] #19: Fullscreen/return, stalled-frame handling, bounded queues, and hardware decode evaluation.

Validation: selections and fullscreen transitions leave recording untouched; no more than four permanent decoders by default; stalled cameras do not hang the UI; bounded memory; Windows/Linux launch and measured four-preview Toughpad CPU/GPU performance.

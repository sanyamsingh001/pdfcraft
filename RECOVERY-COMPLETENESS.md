# Recovery completeness

All **480 registered worktrees** were captured successfully as untested source snapshots, including staged/working variants and local branch/tag tip source associations. Identical trees were deduplicated.

**Unrecovered proposals:** 823 separate proposal paths were unavailable when read. The AP-only proposal forms-ap-only-native4-50f-20261018-v1.unrun.patch is explicitly unrecovered. These omissions are listed in RECOVERY.json; some historical patches may duplicate captured source, but that equivalence was not assumed. No vanished source was reconstructed.

This is source preservation, not validation. No feature PR, CI, native test or upstream merge was performed. Parentless snapshots preserve source content and associations, not original Git ancestry. Credentials, private capture/evidence, logs, build artifacts and caches are excluded.

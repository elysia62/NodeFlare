# v1.0.3 backup fixture

`v1.0.3-backup.zip` was exported with the unchanged `backup::export_archive`
implementation and migrations from tag `v1.0.3` (commit `58dea92`). A temporary
Rust test populated an in-memory SQLite database and called that exporter.
The archive uses backup format 2 and contains only synthetic data.

It includes settings, a hidden free node, latest metrics, both a snapshot and an
aggregated history row, a completed remote task, enabled TOTP, and a theme with
an asset. The redundant SQL CPU value is deliberately 99 while `latest_json`
contains 42.5, so tests verify that the complete JSON report is preserved.

The fake administrator is `fixture-admin` with password `FixturePassword123`;
the TOTP secret is `JBSWY3DPEHPK3PXP`. The primary Agent token is
`fixture-primary-token`. An extra `fixture-install-token` was created in the
source database but is excluded by v1.0.3's export policy.

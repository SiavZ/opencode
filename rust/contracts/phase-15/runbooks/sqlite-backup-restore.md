# Runbook: WAL-safe SQLite backup and restore

Applies to `opencode.db` (`$XDG_DATA_HOME/opencode/opencode.db`, `PRAGMA journal_mode = WAL`, periodic `wal_checkpoint(PASSIVE)`, see `packages/core/src/database/database.ts:27-53`), and to any Rust `storage-sqlite` database that uses WAL.

## The hazard (reproduced 2026-10-03)

In WAL mode, committed transactions can live only in `opencode.db-wal` until a checkpoint runs. Copying only `opencode.db` while a writer has the DB open loses them. Reproduced with Python `sqlite3` and SQLite 3.51.0 in a temp dir, with `wal_autocheckpoint=0`, 100 committed rows and an open connection:

- `cp live.db naive.db` gives `no such table: t` (the schema and every row were lost).
- The online backup API gives `integrity_check = ok`, 100 rows.

Never use `cp`/`rsync`/volume snapshots of the main file alone while opencode runs.

## Backup (online, process may keep running)

Pick one:

```sh
sqlite3 "$DB" ".backup '$OUT/opencode-$(date -u +%Y%m%dT%H%M%SZ).db'"   # online backup API
sqlite3 "$DB" "VACUUM INTO '$OUT/opencode-<ts>.db'"                      # compacted copy
```

Both read a consistent snapshot that includes WAL content. Also copy `auth.json` and keep its 0600 mode. It holds credentials, so encrypt the backup at rest.

## Backup (offline)

Stop opencode, then copy `opencode.db`, `opencode.db-wal` and `opencode.db-shm` together. Better: open the DB once with `sqlite3 "$DB" "PRAGMA wal_checkpoint(TRUNCATE);"` and copy the main file only after that.

## Verify a backup

```sh
sqlite3 backup.db "PRAGMA integrity_check;"        # expect: ok
sqlite3 backup.db "PRAGMA foreign_key_check;"      # expect: no rows
sqlite3 backup.db "select count(*) from session;"  # compare with live
```

## Restore

1. Stop every opencode process and container that uses the data dir (`pgrep opencode`, `docker ps`).
2. Move the current files aside: `opencode.db`, `opencode.db-wal`, `opencode.db-shm` to `*.broken-<ts>`. **A stale `-wal` next to a restored main file corrupts it or replays foreign pages**, so it must not stay.
3. Copy the backup to `opencode.db` and set owner/mode to match the service user (uid 10001 in the container).
4. Run `PRAGMA integrity_check` on the restored file before starting.
5. Start opencode and check that sessions list and health is 200.

Container: run steps 2-4 in a one-off container on the same volume as the image's `opencode` user, for example `docker run --rm -v opencode-data:/data --entrypoint sh opencode-rust:<ver> -c '...'`. `sqlite3` is not in the runtime image, so copy the backup in and check it on the host.

## What is untested

- Exercised: the hazard, `.backup`, `VACUUM INTO`, and restore with a stale WAL removed, all against a synthetic temp DB (SQLite CLI 3.51.0). Not run against a real opencode DB or the Rust `storage-sqlite` schema.
- Backup under concurrent heavy write load, and very large DBs (the backup API restarts if the source changes mid-copy).
- Restore inside the container (no running server yet).
- Encrypted backup storage, retention and deletion schedules.

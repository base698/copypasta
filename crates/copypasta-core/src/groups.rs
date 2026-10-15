//! Groups: every herdr workspace gets one automatically, and users can make
//! their own. A clip belongs to at most one group:
//!
//! * automatically, the herdr workspace it was first copied in; a clip copied
//!   outside herdr (browser, Slack, ...) joins the first workspace it is
//!   pasted into;
//! * manually, whatever the user picked (including "no group"); automatic
//!   grouping never overrides a manual choice.
//!
//! A clip from workspace A pasted into workspace B stays in A, and is listed
//! in B as *incoming*.

use crate::db::Store;
use crate::model::*;
use anyhow::{bail, Result};
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;

const HERDR_PREFIX: &str = "herdr:";

fn group_from_row(r: &rusqlite::Row) -> rusqlite::Result<Group> {
    let key: Option<String> = r.get(2)?;
    Ok(Group {
        id: r.get(0)?,
        name: r.get(1)?,
        workspace: key.and_then(|k| k.strip_prefix(HERDR_PREFIX).map(str::to_string)),
        created: r.get(3)?,
    })
}

impl Store {
    /// The automatic group for a herdr workspace label, created on demand.
    pub fn herdr_group(&mut self, workspace: &str) -> Result<i64> {
        let key = format!("{HERDR_PREFIX}{workspace}");
        if let Some(id) = self
            .conn
            .query_row("SELECT id FROM groups WHERE source_key = ?1", [&key], |r| r.get(0))
            .optional()?
        {
            return Ok(id);
        }
        self.conn.execute(
            "INSERT INTO groups(name, source_key, created) VALUES (?1, ?2, ?3)",
            params![workspace, key, now_ms()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Where a clip belongs automatically: the workspace of its earliest herdr
    /// copy, else of its earliest herdr paste.
    pub fn derived_group(&mut self, clip_id: i64) -> Result<Option<i64>> {
        let ws: Option<String> = self
            .conn
            .query_row(
                "SELECT workspace FROM events WHERE clip_id = ?1 AND workspace IS NOT NULL
                 ORDER BY kind = 'copy' DESC, ts ASC LIMIT 1",
                [clip_id],
                |r| r.get(0),
            )
            .optional()?;
        ws.map(|w| self.herdr_group(&w)).transpose()
    }

    /// Give an ungrouped, non-manual clip its automatic group.
    pub(crate) fn auto_group(&mut self, clip_id: i64) -> Result<()> {
        let (gid, manual): (Option<i64>, i64) = self.conn.query_row(
            "SELECT group_id, group_manual FROM clips WHERE id = ?1",
            [clip_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if gid.is_some() || manual != 0 {
            return Ok(());
        }
        if let Some(g) = self.derived_group(clip_id)? {
            self.conn.execute("UPDATE clips SET group_id = ?2 WHERE id = ?1", params![clip_id, g])?;
        }
        Ok(())
    }

    /// Backfill automatic groups for every clip (used by the migration).
    pub(crate) fn regroup_all(&mut self) -> Result<()> {
        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare("SELECT id FROM clips WHERE group_id IS NULL AND group_manual = 0")?;
            let v = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            v
        };
        for id in ids {
            self.auto_group(id)?;
        }
        Ok(())
    }

    /// Fill in event workspaces written by an older build (they only lived in
    /// the context JSON) and group whatever that makes groupable. Cheap; the
    /// app runs it at startup.
    pub fn repair(&mut self) -> Result<usize> {
        let n = self.conn.execute(
            "UPDATE events SET workspace = json_extract(ctx, '$.herdr.workspace')
             WHERE workspace IS NULL AND json_extract(ctx, '$.herdr.workspace') IS NOT NULL",
            [],
        )?;
        if n > 0 {
            self.regroup_all()?;
        }
        Ok(n)
    }

    pub fn group(&self, id: i64) -> Result<Option<Group>> {
        Ok(self
            .conn
            .query_row("SELECT id, name, source_key, created FROM groups WHERE id = ?1", [id], group_from_row)
            .optional()?)
    }

    pub fn all_groups(&self) -> Result<Vec<Group>> {
        let mut stmt = self.conn.prepare("SELECT id, name, source_key, created FROM groups ORDER BY lower(name)")?;
        let v = stmt.query_map([], group_from_row)?.collect::<rusqlite::Result<_>>()?;
        Ok(v)
    }

    pub(crate) fn clip_group(&self, clip_id: i64) -> Result<Option<GroupRef>> {
        Ok(self
            .conn
            .query_row(
                "SELECT g.id, g.name, c.group_manual FROM clips c JOIN groups g ON g.id = c.group_id WHERE c.id = ?1",
                [clip_id],
                |r| Ok(GroupRef { id: r.get(0)?, name: r.get(1)?, manual: r.get::<_, i64>(2)? != 0 }),
            )
            .optional()?)
    }

    /// Put clips in a group (`None` = no group). Manual: auto-grouping won't
    /// move them again until `reset_group`.
    pub fn move_clips(&mut self, ids: &[i64], group: Option<i64>) -> Result<()> {
        if let Some(g) = group {
            if self.group(g)?.is_none() {
                bail!("no such group {g}");
            }
        }
        let tx = self.conn.transaction()?;
        for id in ids {
            tx.execute("UPDATE clips SET group_id = ?2, group_manual = 1 WHERE id = ?1", params![id, group])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Back to automatic grouping.
    pub fn reset_group(&mut self, id: i64) -> Result<()> {
        self.conn.execute("UPDATE clips SET group_id = NULL, group_manual = 0 WHERE id = ?1", [id])?;
        self.auto_group(id)
    }

    pub fn create_group(&mut self, name: &str) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            bail!("group name is empty");
        }
        self.conn.execute(
            "INSERT INTO groups(name, source_key, created) VALUES (?1, NULL, ?2)",
            params![name, now_ms()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Renaming keeps an automatic group tied to its herdr workspace.
    pub fn rename_group(&mut self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            bail!("group name is empty");
        }
        if self.conn.execute("UPDATE groups SET name = ?2 WHERE id = ?1", params![id, name])? == 0 {
            bail!("no such group {id}");
        }
        Ok(())
    }

    /// Delete a group; its clips become (manually) ungrouped, nothing else is lost.
    pub fn delete_group(&mut self, id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("UPDATE clips SET group_id = NULL, group_manual = 1 WHERE group_id = ?1", [id])?;
        tx.execute("DELETE FROM groups WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    /// Groups with how many clips match `text`, most recently active first.
    /// With an empty search, empty groups are listed too (as drop targets).
    pub fn group_summaries(&self, text: &str) -> Result<Vec<GroupSummary>> {
        let f = self.filter(text)?;
        let groups: HashMap<i64, Group> = self.all_groups()?.into_iter().map(|g| (g.id, g)).collect();
        let mut out: HashMap<Option<i64>, GroupSummary> = HashMap::new();
        {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT c.group_id, COUNT(*), MAX(c.last_seen) FROM clips c WHERE {} GROUP BY c.group_id",
                f.sql
            ))?;
            let rows = stmt.query_map(rusqlite::params_from_iter(f.args.iter()), |r| {
                Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
            })?;
            for row in rows {
                let (gid, count, last) = row?;
                let g = gid.and_then(|id| groups.get(&id));
                out.insert(
                    gid,
                    GroupSummary {
                        id: gid,
                        name: g.map(|g| g.name.clone()).unwrap_or_else(|| "Ungrouped".into()),
                        workspace: g.and_then(|g| g.workspace.clone()),
                        count,
                        incoming: 0,
                        last_seen: last,
                    },
                );
            }
        }
        // Incoming: clips of other groups pasted into a workspace group.
        {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT g.id, COUNT(DISTINCT c.id), MAX(e.ts)
                 FROM events e JOIN clips c ON c.id = e.clip_id
                 JOIN groups g ON g.source_key = '{HERDR_PREFIX}' || e.workspace
                 WHERE e.kind = 'paste' AND (c.group_id IS NULL OR c.group_id != g.id) AND {}
                 GROUP BY g.id",
                f.sql
            ))?;
            let rows = stmt.query_map(rusqlite::params_from_iter(f.args.iter()), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
            })?;
            for row in rows {
                let (gid, n, ts) = row?;
                let Some(g) = groups.get(&gid) else { continue };
                let e = out.entry(Some(gid)).or_insert_with(|| GroupSummary {
                    id: Some(gid),
                    name: g.name.clone(),
                    workspace: g.workspace.clone(),
                    count: 0,
                    incoming: 0,
                    last_seen: 0,
                });
                e.incoming = n;
                e.last_seen = e.last_seen.max(ts);
            }
        }
        let searching = text.split_whitespace().any(|t| {
            let t = t.to_lowercase();
            !(t.starts_with("g:") || t.starts_with("group:"))
        });
        let name_filter: Vec<String> = text
            .split_whitespace()
            .filter_map(|t| {
                let t = t.to_lowercase();
                t.strip_prefix("group:").or_else(|| t.strip_prefix("g:")).map(str::to_string)
            })
            .collect();
        if !searching {
            for g in groups.values() {
                let lname = g.name.to_lowercase();
                if name_filter.iter().all(|n| lname.contains(n)) {
                    out.entry(Some(g.id)).or_insert_with(|| GroupSummary {
                        id: Some(g.id),
                        name: g.name.clone(),
                        workspace: g.workspace.clone(),
                        count: 0,
                        incoming: 0,
                        last_seen: g.created,
                    });
                }
            }
        }
        let mut v: Vec<GroupSummary> = out.into_values().collect();
        v.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(a.name.cmp(&b.name)));
        Ok(v)
    }
}

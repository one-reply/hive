//! One review inventory per tab. Only the selected and visible patches are read.
use super::git::{DiffRequest, Git, GitEvent};
use gpui_kit::component::diff::DiffFile;
use gpui_kit::*;
use hive_core::git::{
    diff::{self, DiffSource, FileDiff},
    reviewed::Revision,
};
use std::{collections::HashMap, sync::Arc};

pub(crate) mod catalog;
use catalog::Entry;

const PARALLEL_READS: usize = 2;
const PATCH_CACHE_BYTES: usize = 24 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Block {
    pub entry: Entry,
    pub file: DiffFile,
    pub patch: Option<Arc<FileDiff>>,
    pub revision: Option<Revision>,
    error: Option<String>,
    checked: Option<u64>,
}

impl Block {
    fn new(entry: Entry, key: &SharedString) -> Self {
        Self {
            file: DiffFile::notice(key.clone(), "Loading changes…"),
            entry,
            patch: None,
            revision: None,
            error: None,
            checked: None,
        }
    }

    fn install(&mut self, key: &SharedString, result: Result<Option<Loaded>, String>) -> bool {
        match result {
            Ok(Some((file, patch, revision))) => {
                self.file = file;
                self.patch = Some(patch);
                self.revision = revision;
                self.error = None;
            }
            Err(error) if self.error.as_ref() != Some(&error) => {
                tracing::warn!(path = %self.entry.request.path, %error, "diff preview failed");
                self.file = DiffFile::notice(key.clone(), error.clone());
                self.patch = None;
                self.revision = None;
                self.error = Some(error);
            }
            _ => return false,
        }
        true
    }
}

pub(crate) struct GitDiff {
    pub request: DiffRequest,
    pub order: Vec<SharedString>,
    pub blocks: HashMap<SharedString, Block>,
    pub generation: u64,
    pub error: Option<String>,
    pub listed: bool,
    active: bool,
    visible: Vec<SharedString>,
    epoch: u64,
    pending: bool,
    git: Entity<Git>,
    inventory: Option<Task<()>>,
    patches: HashMap<SharedString, Task<()>>,
    _subscription: Subscription,
}

impl GitDiff {
    pub fn new(request: DiffRequest, git: Entity<Git>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&git, |this, git, event, cx| {
            if let GitEvent::Refreshed { target, changed } = event
                && this.active
                && git.read(cx).window_active
                && target == &this.request.target
            {
                match this.request.source {
                    DiffSource::Commit(_) if this.listed && this.error.is_none() => (),
                    ref source
                        if catalog::local(source)
                            && !changed
                            && this.listed
                            && this.error.is_none() =>
                    {
                        this.epoch += 1;
                        this.load_visible(cx);
                    }
                    _ => this.refresh(cx),
                }
            }
        });
        Self {
            request,
            git,
            order: Vec::new(),
            blocks: HashMap::new(),
            visible: vec![],
            generation: 0,
            epoch: 0,
            listed: false,
            active: false,
            pending: false,
            error: None,
            inventory: None,
            patches: HashMap::new(),
            _subscription: subscription,
        }
    }

    pub fn select(&mut self, request: DiffRequest, cx: &mut Context<Self>) {
        if !self.request.same_review(&request) {
            self.inventory = None;
            self.patches.clear();
            self.blocks.clear();
            self.order.clear();
            self.visible.clear();
            self.generation += 1;
            self.pending = false;
            self.listed = false;
            self.error = None;
        }
        self.request = request;
        if !self.listed || !self.blocks.contains_key(&self.request.block_key()) {
            self.refresh(cx);
        } else {
            self.load_visible(cx);
        }
        cx.notify();
    }

    pub fn set_visible(&mut self, visible: Vec<SharedString>, cx: &mut Context<Self>) {
        self.visible = visible;
        self.load_visible(cx);
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.refresh(cx);
        } else {
            self.patches.clear();
            self.inventory = None;
            self.pending = false;
        }
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        if catalog::local(&self.request.source)
            && self.git.read(cx).target.as_ref() == Some(&self.request.target)
        {
            self.git.update(cx, |git, cx| git.refresh(true, cx));
        } else {
            self.refresh(cx);
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        if self.inventory.is_some() {
            self.pending = true;
            return;
        }
        let git = self.git.read(cx);
        if catalog::local(&self.request.source)
            && git.target.as_ref() == Some(&self.request.target)
            && let Some(status) = &git.status
        {
            let entries =
                catalog::entries(&self.request.target, &self.request.source, &status.files);
            self.install(Ok(entries), cx);
            return;
        }
        let request = self.request.clone();
        self.inventory = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { catalog::read(&request) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.inventory = None;
                this.install(result, cx);
                if std::mem::take(&mut this.pending) {
                    this.refresh(cx);
                }
            });
        }));
    }

    fn install(&mut self, result: Result<Vec<Entry>, String>, cx: &mut Context<Self>) {
        let first_result = !self.listed;
        self.listed = true;
        let entries = match result {
            Ok(entries) => entries,
            Err(error) => {
                if self.error.as_ref() != Some(&error) {
                    tracing::warn!(%error, "cannot list files for review");
                    self.error = Some(error);
                    cx.notify();
                }
                return;
            }
        };
        let mut changed = self.error.take().is_some() || first_result;
        self.epoch += 1;
        let mut blocks = HashMap::with_capacity(entries.len());
        let mut order = Vec::with_capacity(entries.len());
        for entry in entries {
            let key = entry.request.block_key();
            let block = match self.blocks.remove(&key) {
                Some(mut block) if block.entry.request == entry.request => {
                    changed |= block.entry != entry;
                    block.entry = entry;
                    block
                }
                _ => {
                    changed = true;
                    Block::new(entry, &key)
                }
            };
            blocks.insert(key.clone(), block);
            order.push(key);
        }
        changed |= self.order != order;
        self.order = order;
        self.blocks = blocks;
        if changed {
            self.generation += 1;
            cx.notify();
        }
        self.load_visible(cx);
    }

    fn load_visible(&mut self, cx: &mut Context<Self>) {
        if !self.active || self.error.is_some() || self.patches.len() >= PARALLEL_READS {
            return;
        }
        let wanted = self.request.block_key();
        let epoch = self.epoch;
        let mut candidates = std::iter::once(&wanted)
            .chain(self.visible.iter().filter(|key| *key != &wanted))
            .filter_map(|key| {
                let block = self.blocks.get(key)?;
                (block.checked != Some(epoch) && !self.patches.contains_key(key))
                    .then_some((key.clone(), block.checked))
            })
            .collect::<Vec<_>>();
        // An SSH read can outlive a poll. Give unloaded and older visible files their turn.
        candidates.sort_by_key(|(key, checked)| (*checked, key != &wanted));
        for (key, _) in candidates
            .into_iter()
            .take(PARALLEL_READS - self.patches.len())
        {
            let block = &self.blocks[&key];
            let request = block.entry.request.clone();
            let previous = block.patch.clone();
            let reading = key.clone();
            let reading_key = key.clone();
            let task = cx.spawn(async move |this, cx| {
                let (request, result) = cx
                    .background_executor()
                    .spawn(async move {
                        let result = read_block(&request, &reading, previous.as_deref());
                        (request, result)
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.patches.remove(&key);
                    if let Some(block) = this.blocks.get_mut(&key)
                        && block.entry.request == request
                    {
                        block.checked = Some(epoch);
                        if block.install(&key, result) {
                            this.trim_cache();
                            this.generation += 1;
                            cx.notify();
                        }
                    }
                    this.load_visible(cx);
                });
            });
            self.patches.insert(reading_key, task);
        }
    }

    fn trim_cache(&mut self) {
        let mut bytes: usize = self
            .blocks
            .values()
            .filter_map(|b| b.patch.as_ref())
            .map(|p| p.patch().len())
            .sum();
        if bytes <= PATCH_CACHE_BYTES {
            return;
        }
        let selected = self.request.block_key();
        let mut candidates = self
            .blocks
            .iter()
            .filter(|(key, block)| {
                block.patch.is_some() && *key != &selected && !self.visible.contains(key)
            })
            .map(|(key, block)| (block.checked, key.clone()))
            .collect::<Vec<_>>();
        candidates.sort_unstable();
        for (_, key) in candidates {
            let block = self
                .blocks
                .get_mut(&key)
                .expect("cache candidate still belongs to the review");
            if let Some(patch) = block.patch.take() {
                bytes -= patch.patch().len();
            }
            *block = Block::new(block.entry.clone(), &key);
            if bytes <= PATCH_CACHE_BYTES {
                break;
            }
        }
    }
}

type Loaded = (DiffFile, Arc<FileDiff>, Option<Revision>);

fn read_block(
    request: &DiffRequest,
    key: &SharedString,
    previous: Option<&FileDiff>,
) -> Result<Option<Loaded>, String> {
    let target = &request.target;
    let patch = diff::read(
        &target.host,
        &target.path,
        &request.path,
        &request.source,
        request.old_path.as_deref(),
    )
    .map_err(|e| e.to_string())?;
    let Some(files) = changed_display(&patch, previous)? else {
        return Ok(None);
    };
    // A rename with further edits can match both literal pathspecs. Never apply an
    // ambiguous patch or attach its selection to an unrelated path.
    let file = match files.as_slice() {
        [] => DiffFile::notice(key.clone(), "No changes in this file."),
        [file] => file.clone().with_review_key(key.clone()),
        _ => return Err("This patch contains multiple files. Review it in the terminal.".into()),
    };
    let revision = Revision::from_diff(
        &target.host,
        &target.path,
        &request.path,
        &request.source,
        &patch,
    );
    Ok(Some((file, Arc::new(patch), revision)))
}

fn changed_display(
    patch: &FileDiff,
    previous: Option<&FileDiff>,
) -> Result<Option<Vec<DiffFile>>, String> {
    if previous == Some(patch) {
        return Ok(None);
    }
    parse_patch(patch.patch()).map(Some)
}

/// Renderable Git files for either a normal diff tab or a pipeline's selected file.
pub(crate) fn parse_patch(patch: &str) -> Result<Vec<DiffFile>, String> {
    // Git describes an unmerged file with a combined diff, which has no two-sided display.
    if ["diff --cc ", "diff --combined "]
        .iter()
        .any(|header| patch.starts_with(header))
    {
        return Err(
            "This file has merge conflicts. Open it from Conflicts in the Git panel.".into(),
        );
    }
    DiffFile::parse(patch)
        .map_err(|e| format!("Cannot display this patch: {e}. Open it in the terminal."))
}

#[cfg(test)]
mod tests;

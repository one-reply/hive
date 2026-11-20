//! What an accepted run hands on: the text of a pull request for a lane's branch.
use super::{Report, Run, Stage};
use std::fmt::Write as _;

/// Hosting sites cut longer titles in their lists.
const TITLE_CHARS: usize = 72;

impl Run {
    /// Verified web address of a request created or discovered for this run.
    pub fn request_url(&self, lane: usize) -> Option<&str> {
        self.requests
            .iter()
            .find(|(i, _)| *i == lane)
            .and_then(|(_, url)| crate::git::pull_request::web_url(url))
    }

    /// Remember the hosting service's permanent address for a delivered checkout.
    pub fn record_request(&mut self, lane: usize, url: &str) {
        if self.phase == super::Phase::Accepted
            && self.delivery_lanes().contains(&lane)
            && crate::git::pull_request::web_url(url).is_some()
        {
            self.requests.retain(|(i, _)| *i != lane);
            self.requests.push((lane, url.to_owned()));
        }
    }

    /// Representative lane for each checkout containing assigned work.
    pub fn delivery_lanes(&self) -> Vec<usize> {
        let mut seen = std::collections::HashSet::new();
        self.lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| {
                !lane.assignment.trim().is_empty()
                    && !lane.worktree.as_os_str().is_empty()
                    && seen.insert(&lane.worktree)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Title and Markdown description of a pull request for `lane`'s branch: the task, the
    /// lane's last development report and the evidence QA left. `None` for a lane the plan
    /// gave no work.
    pub fn pull_request_text(&self, lane: usize) -> Option<(String, String)> {
        self.lanes
            .get(lane)
            .filter(|lane| !lane.assignment.trim().is_empty())?;
        let task = self.task.trim();
        let line = task.lines().next().unwrap_or_default().trim_end();
        let mut title: String = line.chars().take(TITLE_CHARS).collect();
        if title.len() < line.len() {
            title.push('…');
        }
        let mut body = format!("## Task\n\n{task}\n");
        let included = |index: usize| {
            index == lane
                || self.lanes.get(index).is_some_and(|l| {
                    !l.assignment.is_empty()
                        && !l.worktree.as_os_str().is_empty()
                        && l.worktree == self.lanes[lane].worktree
                })
        };
        let members: Vec<_> = (0..self.lanes.len()).filter(|&i| included(i)).collect();
        for &i in &members {
            if let Some((_, Report::Developed { summary })) =
                self.stage_reports(Stage::Develop(i)).next_back()
            {
                let label = if members.len() > 1 {
                    format!("Changes · {}", self.lanes[i].name)
                } else {
                    "Changes".into()
                };
                let _ = write!(body, "\n## {label}\n\n{}\n", summary.trim());
            }
        }
        let mut evidence = Vec::new();
        for result in self.commands.iter().filter(|result| included(result.lane)) {
            evidence.push(format!(
                "- `{}`: {}",
                result.command,
                if result.success { "passed" } else { "failed" }
            ));
        }
        for (index, review) in self.template.reviews.iter().enumerate() {
            if let Some((_, Report::Review { summary, .. })) =
                self.stage_reports(Stage::Review(index)).next_back()
            {
                evidence.push(format!("- {}: {}", review.name, summary.trim()));
            }
        }
        if !evidence.is_empty() {
            let _ = write!(body, "\n## Verification\n\n{}\n", evidence.join("\n"));
        }
        let notes: Vec<_> = self
            .notes
            .iter()
            .filter(|(_, note)| included(note.lane))
            .map(|(_, note)| format!("- {}", note.description))
            .collect();
        if !notes.is_empty() {
            let _ = write!(body, "\n## Open reviewer notes\n\n{}\n", notes.join("\n"));
        }
        Some((title, body))
    }
}

#[cfg(test)]
mod tests {
    use crate::pipeline::{
        Assignment, CommandResult, Finding, Lane, Report, Run, Severity, Template,
    };

    fn lane(name: &str) -> Lane {
        Lane {
            name: name.into(),
            project: std::path::Path::new("/").join(name),
            ..Default::default()
        }
    }

    #[test]
    fn a_pull_request_carries_the_task_the_lanes_report_and_the_qa_evidence() {
        let task = format!("Add login {}\nUse the session cookie.", "x".repeat(80));
        let mut lanes = vec![lane("api"), lane("web")];
        lanes[0].command = "cargo test".into();
        let mut run = Run::new(1, "/api".into(), task.clone(), Template::default(), lanes).unwrap();
        let report = |run: &mut Run, value: Report| {
            let ticket = run.pending()[0];
            run.report(&ticket, value).unwrap();
        };
        let assignments = vec![Assignment {
            lane: 0,
            task: "Add the endpoint".into(),
            criteria: "Tests pass".into(),
        }];
        report(&mut run, Report::Plan { assignments });
        run.approve().unwrap();
        // Nothing is developed yet; the request still names the task.
        let (title, body) = run.pull_request_text(0).unwrap();
        assert_eq!(title.chars().count(), 73);
        assert!(title.starts_with("Add login x") && title.ends_with('…'));
        assert_eq!(body, format!("## Task\n\n{task}\n"));
        assert_eq!(run.pull_request_text(1), None);
        assert_eq!(run.pull_request_text(9), None);

        let summary = "Added POST /login and its tests.\n".into();
        report(&mut run, Report::Developed { summary });
        run.command_results(vec![CommandResult {
            lane: 0,
            command: "cargo test".into(),
            success: true,
            exit_code: Some(0),
            output: String::new(),
        }])
        .unwrap();
        let note = Finding {
            lane: 0,
            severity: Severity::Minor,
            description: "Rename the handler".into(),
        };
        let review = |findings| Report::Review {
            evidence: vec![],
            summary: "Criteria hold".into(),
            findings,
        };
        report(&mut run, review(vec![note]));
        report(&mut run, review(vec![]));
        run.accept().unwrap();
        let (_, body) = run.pull_request_text(0).unwrap();
        let sections: Vec<_> = body.split("\n## ").skip(1).collect();
        assert_eq!(
            sections,
            [
                "Changes\n\nAdded POST /login and its tests.\n",
                "Verification\n\n- `cargo test`: passed\n- QA: Criteria hold\n- Security & performance: Criteria hold\n",
                "Open reviewer notes\n\n- Rename the handler\n",
            ]
        );
    }
}

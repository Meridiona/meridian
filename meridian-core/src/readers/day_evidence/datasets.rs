//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The named datasets a summary panel may bind to, and their field lists.
//!
//! # Why this is its own module
//! Three things must agree exactly or the feature breaks in a way that is hard to
//! see: the prompt (which tells the model what it may reference), the validator
//! (which rejects anything else), and the collector (which produces the rows). A
//! single declaration here is what keeps them honest — add a field in one place
//! and all three follow.
//!
//! # Related
//! - [`super::collect`] — builds the rows for each of these (the parent module).
//! - The daemon's `day_summary::validate` — rejects a spec naming anything not
//!   declared here.

/// One dataset the model may bind a panel to.
pub struct Dataset {
    /// The name a spec references: `{"data": {"name": "segments"}}`.
    pub name: &'static str,
    /// What it holds, in the model's terms. Goes into the prompt verbatim.
    pub what: &'static str,
    /// Every field a spec may encode. A field outside this list is a drop.
    pub fields: &'static [(&'static str, &'static str)],
}

/// The full set. Order is the order the prompt lists them in.
pub const DATASETS: &[Dataset] = &[
    Dataset {
        name: "workstreams",
        what: "One row per workstream (a distinct thread of work) inferred for the day.",
        fields: &[
            ("task_id", "string - stable id within the day, e.g. 'T1'"),
            ("title", "string - what the work was"),
            ("minutes", "number - total minutes spent"),
            ("segment_count", "number - how many separate sittings"),
            (
                "first_min",
                "number - minutes past local midnight of the first sitting",
            ),
            (
                "last_min",
                "number - minutes past local midnight of the last sitting",
            ),
        ],
    },
    Dataset {
        name: "segments",
        what: "One row per continuous sitting. Several rows per workstream; this is \
               where fragmentation and interleaving live.",
        fields: &[
            (
                "task_id",
                "string - which workstream this sitting belongs to",
            ),
            ("title", "string - that workstream's title"),
            ("start_min", "number - minutes past local midnight"),
            ("end_min", "number - minutes past local midnight"),
            ("minutes", "number - length of this sitting"),
        ],
    },
    Dataset {
        name: "apps",
        what: "Time per application, coding-agent tools included.",
        fields: &[
            ("app", "string - application name"),
            ("seconds", "number - time in that app"),
        ],
    },
    Dataset {
        name: "categories",
        what: "Time per kind of work (coding, code_review, meeting, communication, \
               design, documentation, planning, deployment_devops, research, idle_personal).",
        fields: &[
            ("category", "string - the kind of work"),
            ("seconds", "number - time in that category"),
        ],
    },
    Dataset {
        name: "hours",
        what: "One row per local hour of the day, 0..23. Hours with no activity are \
               present with zeros, so a chart over this shows the shape of the whole \
               day rather than only its busy parts.",
        fields: &[
            ("hour", "number - local hour, 0..23"),
            ("focus_s", "number - engaged seconds in that hour"),
            (
                "agent_s",
                "number - seconds a coding agent was working in that hour",
            ),
        ],
    },
];

/// The dataset named `name`, if it exists.
pub fn by_name(name: &str) -> Option<&'static Dataset> {
    DATASETS.iter().find(|d| d.name == name)
}

/// Whether `field` is encodable on dataset `name`.
pub fn has_field(name: &str, field: &str) -> bool {
    by_name(name).is_some_and(|d| d.fields.iter().any(|(f, _)| *f == field))
}

/// The dataset list rendered for the prompt: name, what it is, and its fields.
pub fn describe() -> String {
    let mut out = String::new();
    for d in DATASETS {
        out.push_str(&format!("\n### {}\n{}\nFields:\n", d.name, d.what));
        for (f, desc) in d.fields {
            out.push_str(&format!("  - {f}: {desc}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dataset_has_fields_and_is_findable() {
        for d in DATASETS {
            assert!(!d.fields.is_empty(), "{} has no fields", d.name);
            assert!(by_name(d.name).is_some());
        }
    }

    #[test]
    fn field_lookup_is_scoped_to_its_own_dataset() {
        assert!(has_field("segments", "start_min"));
        // `app` is a field of `apps`, not of `segments` — a spec that encodes it
        // on the wrong dataset must be caught.
        assert!(!has_field("segments", "app"));
        assert!(!has_field("nope", "app"));
    }

    /// The prompt is generated from this list, so it can never drift from what the
    /// validator accepts.
    #[test]
    fn describe_names_every_dataset_and_field() {
        let d = describe();
        for ds in DATASETS {
            assert!(d.contains(ds.name));
            for (f, _) in ds.fields {
                assert!(d.contains(f), "{} missing from prompt text", f);
            }
        }
    }
}

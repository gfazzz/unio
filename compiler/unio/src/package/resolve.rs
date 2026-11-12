//! Resolution of dependencies (language-contracts section 6, T-14): deterministic backtracking
//! over the package identities in sorted order, trying the versions that satisfy every
//! requirement in descending semantic-version order, stable versions only unless a requirement
//! names a prerelease. A package that is locked and not in the requested update set keeps its
//! locked version; one version per identity; a conflict is an error that names the paths that
//! require each version, never two hidden copies.

use std::collections::{BTreeMap, BTreeSet};

use super::semver::{Req, Version};

/// A requirement of a package (or of the root) on another package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirement {
    pub alias: String,
    pub id: String,
    pub req: Option<Req>,
}

/// One version of a package and what it requires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub version: Version,
    pub deps: Vec<Requirement>,
}

/// The versions of a package that exist: from a registry index, or the one version of a
/// directory or an archive.
pub trait Provider {
    fn candidates(&mut self, id: &str) -> Result<Vec<Candidate>, String>;
}

/// The most steps a resolution may take before it stops with an error.
pub const MAX_STEPS: usize = 20_000;

#[derive(Clone)]
struct Need {
    /// Who requires it: "root", then each package and version on the way.
    path: Vec<String>,
    id: String,
    req: Option<Req>,
}

#[derive(Clone)]
struct State {
    chosen: BTreeMap<String, Candidate>,
    needs: Vec<Need>,
}

/// The chosen version of every package reachable from the root requirements.
pub fn resolve(
    root: &[Requirement],
    provider: &mut dyn Provider,
    frozen: &BTreeMap<String, Candidate>,
    unlocked: &BTreeSet<String>,
    root_aliases: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Candidate>, String> {
    let state = State {
        chosen: BTreeMap::new(),
        needs: root
            .iter()
            .map(|r| Need {
                path: vec!["the root package".into()],
                id: r.id.clone(),
                req: r.req.clone(),
            })
            .collect(),
    };
    let mut r = Resolver {
        provider,
        frozen,
        unlocked,
        root_aliases,
        steps: 0,
        cache: BTreeMap::new(),
    };
    r.step(state)?
}

struct Resolver<'a> {
    provider: &'a mut dyn Provider,
    frozen: &'a BTreeMap<String, Candidate>,
    unlocked: &'a BTreeSet<String>,
    root_aliases: &'a BTreeMap<String, String>,
    steps: usize,
    cache: BTreeMap<String, Vec<Candidate>>,
}

fn describe(need: &Need) -> String {
    let req = need
        .req
        .as_ref()
        .map_or("any version".to_string(), |r| r.to_string());
    format!("{} requires {req}", need.path.join(" -> "))
}

impl Resolver<'_> {
    fn is_frozen(&self, id: &str) -> bool {
        self.frozen.contains_key(id) && !self.unlocked.contains(id)
    }

    fn candidates(&mut self, id: &str) -> Result<Vec<Candidate>, String> {
        if self.is_frozen(id) {
            return Ok(vec![self.frozen[id].clone()]);
        }
        if let Some(c) = self.cache.get(id) {
            return Ok(c.clone());
        }
        let mut found = self.provider.candidates(id)?;
        found.sort_by(|a, b| b.version.cmp(&a.version));
        found.dedup_by(|a, b| a.version == b.version);
        self.cache.insert(id.to_string(), found.clone());
        Ok(found)
    }

    fn update_hint(&self, id: &str) -> String {
        let name = self
            .root_aliases
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string());
        format!(
            "; the locked version is kept because {id} is not in the requested update: run \
             `unio update {name}` to allow a broader update"
        )
    }

    /// `Ok(Ok(chosen))` when resolved, `Ok(Err(conflict))` when this branch fails, and `Err`
    /// when the resolution must stop (a provider failed, or too many steps).
    #[allow(clippy::type_complexity)]
    fn step(
        &mut self,
        state: State,
    ) -> Result<Result<BTreeMap<String, Candidate>, String>, String> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(format!(
                "the resolution stopped after {MAX_STEPS} steps without a result"
            ));
        }
        // Every requirement on a chosen package must hold.
        for need in &state.needs {
            if let Some(c) = state.chosen.get(&need.id) {
                if need.req.as_ref().is_some_and(|r| !r.matches(&c.version)) {
                    let others: Vec<String> = state
                        .needs
                        .iter()
                        .filter(|n| n.id == need.id)
                        .map(describe)
                        .collect();
                    return Ok(Err(format!(
                        "{} {} is chosen, but {}: no single version of {} satisfies all of: {}",
                        need.id,
                        c.version,
                        describe(need),
                        need.id,
                        others.join("; ")
                    )));
                }
            }
        }
        let open: BTreeSet<&str> = state
            .needs
            .iter()
            .map(|n| n.id.as_str())
            .filter(|id| !state.chosen.contains_key(*id))
            .collect();
        let Some(id) = open.into_iter().next().map(str::to_string) else {
            return Ok(Ok(state.chosen));
        };
        let needs: Vec<&Need> = state.needs.iter().filter(|n| n.id == id).collect();
        let all = self.candidates(&id)?;
        let fitting: Vec<Candidate> = all
            .iter()
            .filter(|c| {
                needs.iter().all(|n| match &n.req {
                    Some(r) => r.matches(&c.version),
                    // No requirement: a directory or an archive, whose one version is taken,
                    // or any stable version.
                    None => !c.version.is_prerelease() || all.len() == 1,
                })
            })
            .cloned()
            .collect();
        let mut last = None;
        for candidate in fitting {
            let mut next = state.clone();
            let mut path = needs[0].path.clone();
            path.push(format!("{id} {}", candidate.version));
            for dep in &candidate.deps {
                next.needs.push(Need {
                    path: path.clone(),
                    id: dep.id.clone(),
                    req: dep.req.clone(),
                });
            }
            next.chosen.insert(id.clone(), candidate);
            match self.step(next)? {
                Ok(chosen) => return Ok(Ok(chosen)),
                Err(conflict) => last = Some(conflict),
            }
        }
        if let Some(conflict) = last {
            return Ok(Err(conflict));
        }
        let wanted: Vec<String> = needs.iter().map(|n| describe(n)).collect();
        let available: Vec<String> = all.iter().map(|c| c.version.to_string()).collect();
        let mut message = format!(
            "no version of {id} satisfies every requirement: {} (available: {})",
            wanted.join("; "),
            if available.is_empty() {
                "none".to_string()
            } else {
                available.join(", ")
            }
        );
        if self.is_frozen(&id) {
            message.push_str(&self.update_hint(&id));
        }
        Ok(Err(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Registry(BTreeMap<String, Vec<Candidate>>, Vec<String>);

    impl Provider for Registry {
        fn candidates(&mut self, id: &str) -> Result<Vec<Candidate>, String> {
            self.1.push(id.to_string());
            Ok(self.0.get(id).cloned().unwrap_or_default())
        }
    }

    fn v(t: &str) -> Version {
        Version::parse(t).unwrap()
    }

    fn need(id: &str, req: &str) -> Requirement {
        Requirement {
            alias: id.replace('-', "_"),
            id: id.into(),
            req: Some(Req::parse(req).unwrap()),
        }
    }

    fn candidate(version: &str, deps: &[(&str, &str)]) -> Candidate {
        Candidate {
            version: v(version),
            deps: deps.iter().map(|(id, req)| need(id, req)).collect(),
        }
    }

    fn registry(packages: &[(&str, Vec<Candidate>)]) -> Registry {
        Registry(
            packages
                .iter()
                .map(|(id, c)| (id.to_string(), c.clone()))
                .collect(),
            Vec::new(),
        )
    }

    fn run(
        root: &[Requirement],
        reg: &mut Registry,
        frozen: &BTreeMap<String, Candidate>,
        unlocked: &[&str],
    ) -> Result<Vec<(String, String)>, String> {
        let unlocked: BTreeSet<String> = unlocked.iter().map(|s| s.to_string()).collect();
        resolve(root, reg, frozen, &unlocked, &BTreeMap::new()).map(|chosen| {
            chosen
                .into_iter()
                .map(|(id, c)| (id, c.version.to_string()))
                .collect()
        })
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn the_highest_satisfying_stable_version_is_chosen() {
        let mut reg = registry(&[(
            "a",
            vec![
                candidate("1.0.0", &[]),
                candidate("1.4.0", &[]),
                candidate("1.5.0-beta.1", &[]),
                candidate("2.0.0", &[]),
            ],
        )]);
        let got = run(&[need("a", "^1.0")], &mut reg, &BTreeMap::new(), &[]).unwrap();
        assert_eq!(got, pairs(&[("a", "1.4.0")]));
    }

    #[test]
    fn a_diamond_shares_one_version_by_backtracking() {
        let mut reg = registry(&[
            (
                "b",
                vec![
                    candidate("1.0.0", &[("d", "^1.0")]),
                    candidate("1.1.0", &[("d", "^1.2")]),
                ],
            ),
            ("c", vec![candidate("1.0.0", &[("d", ">=1.0, <1.2")])]),
            (
                "d",
                vec![
                    candidate("1.0.0", &[]),
                    candidate("1.1.0", &[]),
                    candidate("1.2.0", &[]),
                ],
            ),
        ]);
        let got = run(
            &[need("b", "^1.0"), need("c", "^1.0")],
            &mut reg,
            &BTreeMap::new(),
            &[],
        )
        .unwrap();
        assert_eq!(
            got,
            pairs(&[("b", "1.0.0"), ("c", "1.0.0"), ("d", "1.1.0")])
        );
    }

    #[test]
    fn incompatible_majors_fail_with_the_paths() {
        let mut reg = registry(&[
            ("b", vec![candidate("1.0.0", &[("d", "^1.0")])]),
            ("c", vec![candidate("1.0.0", &[("d", "^2.0")])]),
            ("d", vec![candidate("1.3.0", &[]), candidate("2.1.0", &[])]),
        ]);
        let error = run(
            &[need("b", "^1"), need("c", "^1")],
            &mut reg,
            &BTreeMap::new(),
            &[],
        )
        .unwrap_err();
        assert!(
            error.contains("the root package -> b 1.0.0 requires ^1.0"),
            "{error}"
        );
        assert!(
            error.contains("the root package -> c 1.0.0 requires ^2.0"),
            "{error}"
        );
    }

    #[test]
    fn locked_entries_are_kept_unless_requested() {
        let mut reg = registry(&[("a", vec![candidate("1.0.0", &[]), candidate("1.3.0", &[])])]);
        let frozen: BTreeMap<String, Candidate> =
            [("a".to_string(), candidate("1.0.0", &[]))].into();
        let got = run(&[need("a", "^1")], &mut reg, &frozen, &[]).unwrap();
        assert_eq!(got, pairs(&[("a", "1.0.0")]));
        assert!(reg.1.is_empty(), "a frozen entry needs no index");
        let got = run(&[need("a", "^1")], &mut reg, &frozen, &["a"]).unwrap();
        assert_eq!(got, pairs(&[("a", "1.3.0")]));
        // A frozen entry that no longer satisfies explains and suggests a broader update.
        let error = run(&[need("a", "^1.2")], &mut reg, &frozen, &[]).unwrap_err();
        assert!(error.contains("unio update a"), "{error}");
    }

    #[test]
    fn the_result_does_not_depend_on_the_order_of_requirements_or_candidates() {
        let make = |reverse: bool| {
            let mut a = vec![
                candidate("1.0.0", &[("c", "^1")]),
                candidate("1.1.0", &[("c", "^1.1")]),
            ];
            let mut c = vec![candidate("1.0.0", &[]), candidate("1.1.0", &[])];
            if reverse {
                a.reverse();
                c.reverse();
            }
            registry(&[
                ("a", a),
                ("c", c),
                ("b", vec![candidate("0.1.0", &[("c", "1.0")])]),
            ])
        };
        let mut roots = vec![need("a", "^1"), need("b", "*")];
        let first = run(&roots, &mut make(false), &BTreeMap::new(), &[]).unwrap();
        roots.reverse();
        let second = run(&roots, &mut make(true), &BTreeMap::new(), &[]).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first,
            pairs(&[("a", "1.1.0"), ("b", "0.1.0"), ("c", "1.1.0")])
        );
    }

    #[test]
    fn a_prerelease_needs_an_explicit_requirement() {
        let mut reg = registry(&[("a", vec![candidate("2.0.0-rc.1", &[])])]);
        assert!(run(&[need("a", "^2")], &mut reg, &BTreeMap::new(), &[]).is_err());
        let got = run(&[need("a", "^2.0.0-rc.1")], &mut reg, &BTreeMap::new(), &[]).unwrap();
        assert_eq!(got, pairs(&[("a", "2.0.0-rc.1")]));
    }

    #[test]
    fn a_pathological_registry_stops_at_the_step_limit() {
        // Each version of x requires a version of y that does not exist: every combination
        // is tried until the limit.
        let xs: Vec<Candidate> = (0..200)
            .map(|i| candidate(&format!("1.{i}.0"), &[("y", "^9")]))
            .collect();
        let ys: Vec<Candidate> = (0..200)
            .map(|i| candidate(&format!("1.{i}.0"), &[("x", "<1.0")]))
            .collect();
        let mut reg = registry(&[("w", vec![candidate("1.0.0", &[])]), ("x", xs), ("y", ys)]);
        let result = run(
            &[need("x", "^1"), need("y", "^1")],
            &mut reg,
            &BTreeMap::new(),
            &[],
        );
        assert!(result.is_err());
    }
}

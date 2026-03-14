use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::model::SkillDeclaration;

/// A cycle was detected in the hard dependency graph.
#[derive(Debug, Clone, PartialEq)]
pub struct CycleError {
    /// Skill names forming the cycle. The last element depends on the first.
    pub path: Vec<String>,
}

impl fmt::Display for CycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dependency cycle detected: {}", self.path.join(" -> "))
    }
}

impl std::error::Error for CycleError {}

/// Errors that can occur when building a dependency graph.
#[derive(Debug, Clone, PartialEq)]
pub enum GraphError {
    /// A skill name appears more than once in the input.
    DuplicateSkill(String),
    /// Multiple skills declare the same artifact type in `produces`.
    ConflictingProducers {
        artifact_type: String,
        first: String,
        second: String,
    },
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::DuplicateSkill(name) => {
                write!(f, "duplicate skill name '{name}'")
            }
            GraphError::ConflictingProducers {
                artifact_type,
                first,
                second,
            } => write!(
                f,
                "artifact type '{artifact_type}' is produced by both '{first}' and '{second}'"
            ),
        }
    }
}

impl std::error::Error for GraphError {}

/// A directed dependency graph computed from skill declarations.
///
/// Edges are derived from the relationship between skills' `requires`/`accepts`
/// fields and other skills' `produces`/`may_produce` fields. The graph enables
/// execution ordering, cycle detection, and blocked-skill identification.
#[derive(Debug)]
pub struct DependencyGraph {
    /// Skill names, indexed for O(1) lookup. All `&str` returns borrow from here.
    skill_names: Vec<String>,
    /// Map from skill name to its index in `skill_names`.
    skill_index: HashMap<String, usize>,
    /// Required artifact types per skill (for `blocked_skills`).
    requires_per_skill: Vec<Vec<String>>,
    /// Hard dependency edges: `hard_deps[i]` = indices of skills that skill i depends on.
    hard_deps: Vec<Vec<usize>>,
    /// Soft dependency edges: `soft_deps[i]` = indices of skills that skill i softly depends on.
    soft_deps: Vec<Vec<usize>>,
    /// Reverse hard edges: `hard_dependents[i]` = indices of skills that depend on skill i.
    hard_dependents: Vec<Vec<usize>>,
    /// Reverse soft edges: `soft_dependents[i]` = indices of skills that softly depend on skill i.
    soft_dependents: Vec<Vec<usize>>,
}

impl DependencyGraph {
    /// Build a dependency graph from skill declarations.
    ///
    /// Validates that skill names are unique and no two skills both `produces` the
    /// same artifact type. Required artifacts with no producer are treated as
    /// external dependencies (no graph edge). Returns `GraphError` on validation
    /// failure.
    pub fn build(skills: &[SkillDeclaration]) -> Result<DependencyGraph, GraphError> {
        let n = skills.len();

        // Index skill names.
        let mut skill_names = Vec::with_capacity(n);
        let mut skill_index = HashMap::with_capacity(n);
        for skill in skills {
            if skill_index.contains_key(&skill.name) {
                return Err(GraphError::DuplicateSkill(skill.name.clone()));
            }
            skill_index.insert(skill.name.clone(), skill_names.len());
            skill_names.push(skill.name.clone());
        }

        // Build producer maps.
        // hard_producer: artifact -> skill index (from `produces`)
        // soft_producer: artifact -> skill indices (from `may_produce`)
        let mut hard_producer: HashMap<String, usize> = HashMap::new();
        let mut soft_producer: HashMap<String, Vec<usize>> = HashMap::new();

        for (idx, skill) in skills.iter().enumerate() {
            for artifact in &skill.produces {
                if let Some(&prev_idx) = hard_producer.get(artifact) {
                    return Err(GraphError::ConflictingProducers {
                        artifact_type: artifact.clone(),
                        first: skill_names[prev_idx].clone(),
                        second: skill.name.clone(),
                    });
                }
                hard_producer.insert(artifact.clone(), idx);
            }
            for artifact in &skill.may_produce {
                // may_produce x may_produce is allowed; all producers get edges.
                soft_producer.entry(artifact.clone()).or_default().push(idx);
            }
        }

        // Resolve edges.
        let mut hard_deps = vec![Vec::new(); n];
        let mut soft_deps = vec![Vec::new(); n];
        let mut hard_dependents = vec![Vec::new(); n];
        let mut soft_dependents = vec![Vec::new(); n];
        let mut requires_per_skill = Vec::with_capacity(n);

        for (idx, skill) in skills.iter().enumerate() {
            requires_per_skill.push(skill.requires.clone());

            // requires -> produces = hard edge
            // requires -> may_produce = soft edge (to all producers)
            // requires -> nothing = external dependency (no edge)
            for artifact in &skill.requires {
                if let Some(&producer_idx) = hard_producer.get(artifact) {
                    if producer_idx != idx {
                        hard_deps[idx].push(producer_idx);
                        hard_dependents[producer_idx].push(idx);
                    }
                } else if let Some(producer_indices) = soft_producer.get(artifact) {
                    for &producer_idx in producer_indices {
                        if producer_idx != idx {
                            soft_deps[idx].push(producer_idx);
                            soft_dependents[producer_idx].push(idx);
                        }
                    }
                }
                // No producer: external dependency — no graph edge needed.
            }

            // accepts -> any producer = soft edge
            // accepts -> nothing = silently ignored
            for artifact in &skill.accepts {
                if let Some(&producer_idx) = hard_producer.get(artifact) {
                    if producer_idx != idx {
                        soft_deps[idx].push(producer_idx);
                        soft_dependents[producer_idx].push(idx);
                    }
                } else if let Some(producer_indices) = soft_producer.get(artifact) {
                    for &producer_idx in producer_indices {
                        if producer_idx != idx {
                            soft_deps[idx].push(producer_idx);
                            soft_dependents[producer_idx].push(idx);
                        }
                    }
                }
            }
        }

        // Deduplicate adjacency lists.
        for idx in 0..n {
            hard_deps[idx].sort_unstable();
            hard_deps[idx].dedup();
            soft_deps[idx].sort_unstable();
            soft_deps[idx].dedup();
            hard_dependents[idx].sort_unstable();
            hard_dependents[idx].dedup();
            soft_dependents[idx].sort_unstable();
            soft_dependents[idx].dedup();
        }

        Ok(DependencyGraph {
            skill_names,
            skill_index,
            requires_per_skill,
            hard_deps,
            soft_deps,
            hard_dependents,
            soft_dependents,
        })
    }

    /// Return skills in a valid execution order.
    ///
    /// Uses Kahn's algorithm on combined hard+soft edges. If a cycle is found
    /// in the combined graph, retries on hard edges only. A cycle in hard edges
    /// returns `CycleError`; a cycle only in soft edges is ignored and the
    /// hard-edge order is returned.
    pub fn topological_order(&self) -> Result<Vec<&str>, CycleError> {
        // Try combined (hard + soft) edges first.
        if let Some(order) = self.kahns_sort(true) {
            return Ok(order
                .iter()
                .map(|&i| self.skill_names[i].as_str())
                .collect());
        }

        // Combined graph has a cycle. Try hard edges only.
        if let Some(order) = self.kahns_sort(false) {
            return Ok(order
                .iter()
                .map(|&i| self.skill_names[i].as_str())
                .collect());
        }

        // Hard edges have a cycle. Extract the cycle path.
        Err(self.extract_cycle())
    }

    /// Return `(skill_name, missing_artifact_types)` for each skill that has
    /// unmet `requires`. Missing artifact types are those in `requires` that
    /// aren't in `available_artifacts`. Results are sorted by skill name.
    pub fn blocked_skills_with_reasons(
        &self,
        available_artifacts: &HashSet<String>,
    ) -> Vec<(&str, Vec<&str>)> {
        let mut result: Vec<(&str, Vec<&str>)> = self
            .requires_per_skill
            .iter()
            .enumerate()
            .filter_map(|(idx, reqs)| {
                let missing: Vec<&str> = reqs
                    .iter()
                    .filter(|r| !available_artifacts.contains(r.as_str()))
                    .map(|r| r.as_str())
                    .collect();
                if missing.is_empty() {
                    None
                } else {
                    Some((self.skill_names[idx].as_str(), missing))
                }
            })
            .collect();
        result.sort_by_key(|(name, _)| *name);
        result
    }

    /// Return skills whose `requires` are not all present in `available_artifacts`.
    pub fn blocked_skills(&self, available_artifacts: &HashSet<String>) -> Vec<&str> {
        self.blocked_skills_with_reasons(available_artifacts)
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// Return the names of skills that this skill directly depends on (hard edges).
    pub fn dependencies_of(&self, skill_name: &str) -> Vec<&str> {
        self.skill_index
            .get(skill_name)
            .map(|&idx| {
                self.hard_deps[idx]
                    .iter()
                    .map(|&dep| self.skill_names[dep].as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Return the names of skills that depend on this skill (hard edges).
    pub fn dependents_of(&self, skill_name: &str) -> Vec<&str> {
        self.skill_index
            .get(skill_name)
            .map(|&idx| {
                self.hard_dependents[idx]
                    .iter()
                    .map(|&dep| self.skill_names[dep].as_str())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Run Kahn's algorithm. Returns `None` if a cycle is detected.
    /// When `include_soft` is true, both hard and soft edges are considered.
    fn kahns_sort(&self, include_soft: bool) -> Option<Vec<usize>> {
        let order = self.kahns_prefix(include_soft);
        if order.len() == self.skill_names.len() {
            Some(order)
        } else {
            None
        }
    }

    /// Run Kahn's algorithm and return the resulting order.
    fn kahns_prefix(&self, include_soft: bool) -> Vec<usize> {
        let n = self.skill_names.len();
        let mut in_degree = vec![0usize; n];

        // Compute in-degrees.
        for (idx, degree) in in_degree.iter_mut().enumerate().take(n) {
            *degree += self.hard_deps[idx].len();
            if include_soft {
                *degree += self.soft_deps[idx].len();
            }
        }

        // Seed queue with zero in-degree nodes.
        let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
        let mut order = Vec::with_capacity(n);

        while let Some(node) = queue.pop() {
            order.push(node);

            // Decrease in-degree for dependents via reverse adjacency lists.
            for &dependent in &self.hard_dependents[node] {
                in_degree[dependent] -= 1;
                if in_degree[dependent] == 0 {
                    queue.push(dependent);
                }
            }
            if include_soft {
                for &dependent in &self.soft_dependents[node] {
                    in_degree[dependent] -= 1;
                    if in_degree[dependent] == 0 {
                        queue.push(dependent);
                    }
                }
            }
        }

        order
    }

    /// Extract a cycle from hard edges using DFS. Called only when a hard cycle exists.
    fn extract_cycle(&self) -> CycleError {
        let n = self.skill_names.len();
        // 0 = unvisited, 1 = in current path, 2 = done
        let mut state = vec![0u8; n];
        let mut path = Vec::new();

        for start in 0..n {
            if state[start] == 0
                && let Some(cycle) = self.dfs_cycle(start, &mut state, &mut path)
            {
                return cycle;
            }
        }

        // Should not reach here if called correctly, but provide a fallback.
        CycleError {
            path: vec!["unknown".into()],
        }
    }

    fn dfs_cycle(
        &self,
        node: usize,
        state: &mut [u8],
        path: &mut Vec<usize>,
    ) -> Option<CycleError> {
        state[node] = 1;
        path.push(node);

        for &dep in &self.hard_deps[node] {
            if state[dep] == 1 {
                // Found a cycle. Extract from `dep` to current position.
                let cycle_start = path.iter().position(|&n| n == dep).unwrap();
                let cycle_path: Vec<String> = path[cycle_start..]
                    .iter()
                    .map(|&i| self.skill_names[i].clone())
                    .collect();
                return Some(CycleError { path: cycle_path });
            }
            if state[dep] == 0
                && let Some(cycle) = self.dfs_cycle(dep, state, path)
            {
                return Some(cycle);
            }
        }

        path.pop();
        state[node] = 2;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TriggerCondition;

    fn skill(name: &str, requires: &[&str], produces: &[&str]) -> SkillDeclaration {
        SkillDeclaration {
            name: name.into(),
            requires: requires.iter().map(|s| (*s).into()).collect(),
            accepts: vec![],
            produces: produces.iter().map(|s| (*s).into()).collect(),
            may_produce: vec![],
            trigger: TriggerCondition::OnSignal {
                name: "manual".into(),
            },
        }
    }

    fn skill_with_may(
        name: &str,
        requires: &[&str],
        produces: &[&str],
        may_produce: &[&str],
    ) -> SkillDeclaration {
        SkillDeclaration {
            name: name.into(),
            requires: requires.iter().map(|s| (*s).into()).collect(),
            accepts: vec![],
            produces: produces.iter().map(|s| (*s).into()).collect(),
            may_produce: may_produce.iter().map(|s| (*s).into()).collect(),
            trigger: TriggerCondition::OnSignal {
                name: "manual".into(),
            },
        }
    }

    fn skill_with_accepts(
        name: &str,
        requires: &[&str],
        accepts: &[&str],
        produces: &[&str],
    ) -> SkillDeclaration {
        SkillDeclaration {
            name: name.into(),
            requires: requires.iter().map(|s| (*s).into()).collect(),
            accepts: accepts.iter().map(|s| (*s).into()).collect(),
            produces: produces.iter().map(|s| (*s).into()).collect(),
            may_produce: vec![],
            trigger: TriggerCondition::OnSignal {
                name: "manual".into(),
            },
        }
    }

    /// Assert that `order` respects the constraint: `before` appears before `after`.
    fn assert_before(order: &[&str], before: &str, after: &str) {
        let pos_before = order.iter().position(|&s| s == before).unwrap_or_else(|| {
            panic!("'{before}' not found in order: {order:?}");
        });
        let pos_after = order.iter().position(|&s| s == after).unwrap_or_else(|| {
            panic!("'{after}' not found in order: {order:?}");
        });
        assert!(
            pos_before < pos_after,
            "expected '{before}' before '{after}', got order: {order:?}"
        );
    }

    // --- Topology tests ---

    #[test]
    fn linear_chain() {
        let skills = vec![
            skill("A", &[], &["X"]),
            skill("B", &["X"], &["Y"]),
            skill("C", &["Y"], &[]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 3);
        assert_before(&order, "A", "B");
        assert_before(&order, "B", "C");
    }

    #[test]
    fn fan_out() {
        let skills = vec![
            skill("A", &[], &["X", "Y"]),
            skill("B", &["X"], &[]),
            skill("C", &["Y"], &[]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 3);
        assert_before(&order, "A", "B");
        assert_before(&order, "A", "C");
    }

    #[test]
    fn fan_in() {
        let skills = vec![
            skill("A", &[], &["X"]),
            skill("B", &[], &["Y"]),
            skill("C", &["X", "Y"], &[]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 3);
        assert_before(&order, "A", "C");
        assert_before(&order, "B", "C");
    }

    #[test]
    fn diamond_dependency() {
        let skills = vec![
            skill("A", &[], &["X"]),
            skill("B", &["X"], &["Y"]),
            skill("C", &["X"], &["Z"]),
            skill("D", &["Y", "Z"], &[]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 4);
        assert_before(&order, "A", "B");
        assert_before(&order, "A", "C");
        assert_before(&order, "B", "D");
        assert_before(&order, "C", "D");
    }

    #[test]
    fn isolated_skill() {
        let skills = vec![
            skill("A", &[], &["X"]),
            skill("B", &["X"], &[]),
            skill("isolated", &[], &[]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 3);
        assert_before(&order, "A", "B");
        assert!(order.contains(&"isolated"));
    }

    #[test]
    fn self_referencing_skill() {
        // A skill that requires an artifact it also produces.
        // The producer_idx != idx guard should prevent a self-edge.
        let skills = vec![skill("A", &["X"], &["X"])];
        let graph = DependencyGraph::build(&skills).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order, vec!["A"]);
        assert!(graph.dependencies_of("A").is_empty());
        assert!(graph.dependents_of("A").is_empty());
    }

    // --- Cycle detection ---

    #[test]
    fn cycle_detection() {
        let skills = vec![skill("A", &["Y"], &["X"]), skill("B", &["X"], &["Y"])];
        let graph = DependencyGraph::build(&skills).unwrap();
        let err = graph.topological_order().unwrap_err();
        assert!(err.path.contains(&"A".to_string()));
        assert!(err.path.contains(&"B".to_string()));
    }

    // --- Soft edges ---

    #[test]
    fn may_produce_creates_soft_ordering() {
        let skills = vec![
            skill_with_may("A", &[], &[], &["X"]),
            skill("B", &["X"], &[]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 2);
        assert_before(&order, "A", "B");
    }

    #[test]
    fn may_produce_cycle_does_not_cause_error() {
        // A may_produce X, B requires X and produces Y, A requires Y.
        // Hard edges: A -> B (A requires Y which B produces).
        // Soft edges: B -> A (B requires X which A may_produce).
        // Combined graph has a cycle but hard graph does not.
        let skills = vec![
            skill_with_may("A", &["Y"], &[], &["X"]),
            skill("B", &["X"], &["Y"]),
        ];
        // B requires X, only A may_produce it -> soft edge B depends on A.
        // A requires Y, B produces it -> hard edge A depends on B.
        // Combined: cycle. Hard only: A -> B, no cycle.
        let graph = DependencyGraph::build(&skills).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 2);
        // Hard order: B before A (A depends on B for Y).
        assert_before(&order, "B", "A");
    }

    // --- Accepts edge ---

    #[test]
    fn accepts_creates_soft_ordering() {
        let skills = vec![
            skill("A", &[], &["X"]),
            skill_with_accepts("B", &[], &["X"], &[]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 2);
        assert_before(&order, "A", "B");
    }

    #[test]
    fn accepts_does_not_block() {
        // B accepts X but nobody produces it. Build should succeed.
        let skills = vec![skill_with_accepts("B", &[], &["X"], &[])];
        let graph = DependencyGraph::build(&skills).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order, vec!["B"]);
    }

    #[test]
    fn accepts_not_in_blocked_skills() {
        let skills = vec![
            skill("A", &[], &["X"]),
            skill_with_accepts("B", &[], &["X"], &[]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();
        // No artifacts available, but B only accepts X (doesn't require it).
        let blocked = graph.blocked_skills(&HashSet::new());
        assert!(!blocked.contains(&"B"));
        assert!(!blocked.contains(&"A")); // A has no requires either
    }

    // --- blocked_skills ---

    #[test]
    fn blocked_skills_with_partial_artifacts() {
        let skills = vec![
            skill("A", &[], &["X"]),
            skill("B", &["X"], &["Y"]),
            skill("C", &["X", "Y"], &[]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();

        // Only X available: B is unblocked, C is blocked (needs Y).
        let available: HashSet<String> = ["X".into()].into();
        let blocked = graph.blocked_skills(&available);
        assert!(!blocked.contains(&"A"));
        assert!(!blocked.contains(&"B"));
        assert!(blocked.contains(&"C"));

        // Both available: nothing blocked.
        let available: HashSet<String> = ["X".into(), "Y".into()].into();
        let blocked = graph.blocked_skills(&available);
        assert!(blocked.is_empty());
    }

    // --- dependencies_of / dependents_of ---

    #[test]
    fn dependencies_and_dependents_diamond() {
        let skills = vec![
            skill("A", &[], &["X"]),
            skill("B", &["X"], &["Y"]),
            skill("C", &["X"], &["Z"]),
            skill("D", &["Y", "Z"], &[]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();

        assert!(graph.dependencies_of("A").is_empty());

        let b_deps = graph.dependencies_of("B");
        assert_eq!(b_deps, vec!["A"]);

        let c_deps = graph.dependencies_of("C");
        assert_eq!(c_deps, vec!["A"]);

        let mut d_deps = graph.dependencies_of("D");
        d_deps.sort();
        assert_eq!(d_deps, vec!["B", "C"]);

        let mut a_dependents = graph.dependents_of("A");
        a_dependents.sort();
        assert_eq!(a_dependents, vec!["B", "C"]);

        assert_eq!(graph.dependents_of("D"), Vec::<&str>::new());
    }

    #[test]
    fn unknown_skill_returns_empty() {
        let skills = vec![skill("A", &[], &["X"])];
        let graph = DependencyGraph::build(&skills).unwrap();
        assert!(graph.dependencies_of("nonexistent").is_empty());
        assert!(graph.dependents_of("nonexistent").is_empty());
    }

    // --- Error cases ---

    #[test]
    fn duplicate_skill_name() {
        let skills = vec![skill("A", &[], &["X"]), skill("A", &[], &["Y"])];
        let err = DependencyGraph::build(&skills).unwrap_err();
        assert_eq!(err, GraphError::DuplicateSkill("A".into()));
    }

    #[test]
    fn conflicting_producers() {
        let skills = vec![skill("A", &[], &["X"]), skill("B", &[], &["X"])];
        let err = DependencyGraph::build(&skills).unwrap_err();
        assert!(matches!(
            err,
            GraphError::ConflictingProducers {
                artifact_type,
                ..
            } if artifact_type == "X"
        ));
    }

    #[test]
    fn may_produce_does_not_conflict_with_produces() {
        let skills = vec![
            skill("A", &[], &["X"]),
            skill_with_may("B", &[], &[], &["X"]),
        ];
        // produces x may_produce is NOT a conflict.
        assert!(DependencyGraph::build(&skills).is_ok());
    }

    #[test]
    fn may_produce_does_not_conflict_with_may_produce() {
        let skills = vec![
            skill_with_may("A", &[], &[], &["X"]),
            skill_with_may("B", &[], &[], &["X"]),
        ];
        assert!(DependencyGraph::build(&skills).is_ok());
    }

    // --- External dependencies ---

    #[test]
    fn external_dependency_builds_successfully() {
        // A requires X but no skill produces X — external dependency.
        let skills = vec![skill("A", &["X"], &[])];
        let graph = DependencyGraph::build(&skills).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order, vec!["A"]);
        assert!(graph.dependencies_of("A").is_empty());
    }

    #[test]
    fn external_dependency_reported_as_blocked() {
        // A requires X (external). blocked_skills should report it when X unavailable.
        let skills = vec![skill("A", &["X"], &[])];
        let graph = DependencyGraph::build(&skills).unwrap();
        let blocked = graph.blocked_skills(&HashSet::new());
        assert_eq!(blocked, vec!["A"]);
        // When X is available, A is unblocked.
        let available: HashSet<String> = ["X".into()].into();
        let blocked = graph.blocked_skills(&available);
        assert!(blocked.is_empty());
    }

    // --- Multiple may_produce producers ---

    #[test]
    fn multiple_may_produce_creates_soft_edges_to_all() {
        // A and B both may_produce X. C requires X.
        // C should have soft edges to both A and B.
        let skills = vec![
            skill_with_may("A", &[], &[], &["X"]),
            skill_with_may("B", &[], &[], &["X"]),
            skill("C", &["X"], &[]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();
        let order = graph.topological_order().unwrap();
        assert_eq!(order.len(), 3);
        // Both A and B should come before C in topo order.
        assert_before(&order, "A", "C");
        assert_before(&order, "B", "C");
    }

    // --- blocked_skills_with_reasons ---

    #[test]
    fn blocked_with_reasons_all_requires_met() {
        let skills = vec![
            skill("A", &[], &["X"]),
            skill("B", &["X"], &["Y"]),
            skill("C", &["X", "Y"], &[]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();
        let available: HashSet<String> = ["X".into(), "Y".into()].into();
        let result = graph.blocked_skills_with_reasons(&available);
        assert!(result.is_empty());
    }

    #[test]
    fn blocked_with_reasons_some_missing() {
        let skills = vec![skill("A", &[], &["X", "Y"]), skill("B", &["X", "Y"], &[])];
        let graph = DependencyGraph::build(&skills).unwrap();
        // Only X available; B needs Y too.
        let available: HashSet<String> = ["X".into()].into();
        let result = graph.blocked_skills_with_reasons(&available);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "B");
        assert_eq!(result[0].1, vec!["Y"]);
    }

    #[test]
    fn blocked_with_reasons_multiple_skills_partial_overlap() {
        let skills = vec![
            skill("A", &[], &["X"]),
            skill("B", &["X", "Y"], &["Y"]),
            skill("C", &["Y", "Z"], &["Z"]),
        ];
        let graph = DependencyGraph::build(&skills).unwrap();
        // Nothing available.
        let available: HashSet<String> = HashSet::new();
        let result = graph.blocked_skills_with_reasons(&available);
        // A has no requires, so not blocked. B missing X,Y. C missing Y,Z.
        assert_eq!(result.len(), 2);
        // Sorted by skill name: B before C.
        assert_eq!(result[0].0, "B");
        assert_eq!(result[0].1, vec!["X", "Y"]);
        assert_eq!(result[1].0, "C");
        assert_eq!(result[1].1, vec!["Y", "Z"]);
    }

    #[test]
    fn blocked_with_reasons_no_skills_have_requires() {
        let skills = vec![skill("A", &[], &["X"]), skill("B", &[], &["Y"])];
        let graph = DependencyGraph::build(&skills).unwrap();
        let result = graph.blocked_skills_with_reasons(&HashSet::new());
        assert!(result.is_empty());
    }

    // --- Display impls ---

    #[test]
    fn error_display_messages() {
        let err = GraphError::DuplicateSkill("test".into());
        assert_eq!(err.to_string(), "duplicate skill name 'test'");

        let err = GraphError::ConflictingProducers {
            artifact_type: "X".into(),
            first: "A".into(),
            second: "B".into(),
        };
        assert_eq!(
            err.to_string(),
            "artifact type 'X' is produced by both 'A' and 'B'"
        );

        let err = CycleError {
            path: vec!["A".into(), "B".into()],
        };
        assert_eq!(err.to_string(), "dependency cycle detected: A -> B");
    }
}

//! BFS  LEVEL    Stage 4 Entity - one Key, one state graph, every body at once  [crate brainos-entity]
//! BFS  PURPOSE  Motor memory on the entity's side: stores each motor primitive in the state graph as a compiled skill, and binds a learned recipe as a belief only after its own success test has passed
//! BFS  DEFINES  Trial
//! BFS  USES     this crate: Entity | brainos: abi, memory, motor
//! BFS  USED BY  entity/entity
//!
//! Motor memory is memory, so it lives here and nowhere else.
//!
//! A body runs a primitive; it does not keep one. The primitive is a
//! skill node in the state graph, written with the entity's own handle,
//! and a body is handed a copy for the length of a run. Moving to a
//! different body of the same class takes nothing with it but the Key.
//!
//! A recipe learned from one demonstration is a guess about the world
//! until it has been tried. `consider_recipe` writes what happened as an
//! episode either way, and writes the recipe as a **belief** only when
//! its own success test passed. A recipe that failed leaves a record
//! that it was tried and no belief that it works.

use super::Entity;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use brainos_abi::codec::{Decode, Decoder, Encode, Encoder};
use brainos_abi::ids::NodeId;
use brainos_memory::graph::{EdgeKind, NodeKind};
use brainos_memory::provenance::Provenance;
use brainos_motor::primitive::MotorPrimitive;
use brainos_motor::recipe::Recipe;

const PRIMITIVE: &str = "motor primitive ";
const RECIPE: &str = "motor recipe ";

/// How a recipe's trial went.
#[derive(Clone, Debug, PartialEq)]
pub struct Trial {
    pub passed: bool,
    /// one line: what the success test saw
    pub detail: String,
}

impl Entity {
    /// Keep a primitive as a compiled skill. `None` if it does not fit
    /// in a skill node; nothing is stored in that case.
    pub fn learn_primitive(&mut self, p: &MotorPrimitive) -> Option<NodeId> {
        let mut e = Encoder::new();
        p.encode(&mut e);
        let bytes = e.finish().ok()?;
        // one node per verb and body class: learning it again replaces it
        let name = format!("{PRIMITIVE}{} for {}", p.verb.as_str(), p.body_class);
        let existing = self
            .graph
            .of_kind(NodeKind::Skill)
            .find(|n| n.content == name)
            .map(|n| n.id);
        let id = match existing {
            Some(id) => id,
            None => self.graph.add(
                &self.handle,
                NodeKind::Skill,
                &name,
                90,
                Provenance::own(self.tick),
            ),
        };
        if self.graph.set_skill_state(&self.handle, id, &bytes) {
            Some(id)
        } else {
            if existing.is_none() {
                self.graph.forget(&self.handle, id);
            }
            None
        }
    }

    /// The primitives this entity knows for a class of body, read back
    /// out of the graph.
    pub fn primitives_for(&self, body_class: &str) -> Vec<MotorPrimitive> {
        self.graph
            .of_kind(NodeKind::Skill)
            .filter(|n| n.content.starts_with(PRIMITIVE))
            .filter_map(|n| {
                let mut d = Decoder::new(&n.state);
                let p = MotorPrimitive::decode(&mut d).ok()?;
                d.finish().ok()?;
                Some(p)
            })
            .filter(|p| p.body_class == body_class)
            .collect()
    }

    /// Record that a recipe was tried, and believe it only if it worked.
    /// Returns the belief, if one was written.
    pub fn consider_recipe(&mut self, name: &str, recipe: &Recipe, trial: &Trial) -> Option<NodeId> {
        let episode = self.graph.add(
            &self.handle,
            NodeKind::Episode,
            &format!(
                "tried recipe {name} ({} steps): {}: {}",
                recipe.steps.len(),
                if trial.passed { "passed" } else { "failed" },
                trial.detail
            ),
            80,
            Provenance::own(self.tick),
        );
        if !trial.passed {
            return None;
        }
        let text = format!("{RECIPE}{name}\n{}", recipe.to_text());
        // a recipe that would be cut short is not a recipe
        if text.len() > self.graph.bounds().max_content {
            return None;
        }
        let belief = self.graph.add(
            &self.handle,
            NodeKind::Belief,
            &text,
            80,
            Provenance::own(self.tick),
        );
        self.graph.link(&self.handle, episode, belief, EdgeKind::Supports);
        Some(belief)
    }

    /// A recipe this entity believes, by name.
    pub fn believed_recipe(&self, name: &str) -> Option<Recipe> {
        let head = format!("{RECIPE}{name}\n");
        self.graph
            .of_kind(NodeKind::Belief)
            .filter(|n| n.content.starts_with(&head))
            .last()
            .and_then(|n| Recipe::from_text(&n.content[head.len()..]))
    }
}

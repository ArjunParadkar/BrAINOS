"""
memory.py -- BrAInOS conversational memory (the Pi embodiment of Domain 5).

The authoritative state graph is the no_std Rust crate `brainos-memory`
(episodic / semantic / procedural / beliefs, consolidation, the
Memory-Integrity Law, provenance on every fact). This is the Pi-side store the
conversational mind reads and writes, built to the same rules so it can later
be backed by the Rust graph unchanged:

  * **Episodic** -- the turns, in order, bounded (a ring). The bound is what
    makes consolidation necessary.
  * **Semantic** -- facts about a person (name, likes, details), each with the
    PROVENANCE that made it true (which turn), a confidence and a repetition
    count. Compressed from episodes; survives their pruning.
  * **Preferences** -- a small keyed store.
  Consolidation ("dreaming") crystallises repeated facts, decays what stopped
  mattering, and keeps everything bounded. Nothing is unbounded; a recall
  carries its provenance.

Persisted to a JSON file (the "flash" leaves go to disk, as in the OS). This
is not a device- or model-resident memory: it belongs to the entity.
"""
from __future__ import annotations

import json
import os
import time
from dataclasses import asdict, dataclass, field


@dataclass
class Fact:
    key: str                    # e.g. "name", "likes", "detail"
    value: str
    provenance: str             # which turn/source made it true
    confidence: float = 0.6
    count: int = 1              # repetitions -> salience
    first_seen: float = field(default_factory=time.time)
    last_seen: float = field(default_factory=time.time)

    def salience(self, now=None):
        now = now or time.time()
        age_days = (now - self.last_seen) / 86400.0
        return self.confidence * (1 + 0.4 * (self.count - 1)) * (0.5 ** (age_days / 14))


class Memory:
    def __init__(self, path=None, max_episodes=200, max_facts_per_person=40):
        self.path = path
        self.max_episodes = max_episodes
        self.max_facts = max_facts_per_person
        self.people: dict[str, dict] = {}     # person -> {"facts":[Fact], "prefs":{}}
        self.episodes: list[dict] = []        # {"t","person","user","blur"}
        self.current_person = "friend"
        if path and os.path.exists(path):
            self.load()

    # ---- people / facts ----
    def _person(self, name):
        return self.people.setdefault(name, {"facts": [], "prefs": {}})

    def set_person(self, name):
        self.current_person = name

    def remember_fact(self, key, value, provenance, person=None, confidence=0.6):
        p = self._person(person or self.current_person)
        for f in p["facts"]:
            if f.key == key and f.value.lower() == value.lower():
                f.count += 1
                f.confidence = min(0.99, f.confidence + 0.1)
                f.last_seen = time.time()
                f.provenance = provenance
                return f
        f = Fact(key, value, provenance, confidence)
        p["facts"].append(f)
        return f

    def set_pref(self, key, value, person=None):
        self._person(person or self.current_person)["prefs"][key] = value

    def name_of(self, person=None):
        p = self._person(person or self.current_person)
        for f in p["facts"]:
            if f.key == "name":
                return f.value
        return None

    # ---- retrieval for the fast path ----
    def retrieve(self, query, person=None, k=4):
        """Top-k salient facts relevant to the query (keyword overlap +
        salience), each with provenance. Small, for a tiny prompt."""
        p = self._person(person or self.current_person)
        q = set(w.strip(".,?!").lower() for w in query.split())
        now = time.time()
        scored = []
        for f in p["facts"]:
            overlap = len(q & set((f.key + " " + f.value).lower().split()))
            scored.append((overlap * 2 + f.salience(now), f))
        scored.sort(key=lambda s: -s[0])
        return [s[1] for s in scored[:k] if s[0] > 0] or [f for _, f in scored[:k]]

    def snippet(self, query, person=None, k=4):
        """A compact memory string for the LLM prompt."""
        name = self.name_of(person)
        facts = self.retrieve(query, person, k)
        bits = ([f"their name is {name}"] if name else []) + \
               [f"{f.key}: {f.value}" for f in facts if f.key != "name"]
        return "; ".join(bits)

    def is_recall_hit(self, query, person=None):
        """True only when memory can answer this recall question directly (so
        the fast path can skip the thinking display). Name questions are the
        clear case; other recall/complex questions fall through to the slow
        path's memory search."""
        ql = query.lower()
        if any(w in ql for w in ("my name", "who am i", "remember me")):
            return self.name_of(person) is not None
        return False

    # ---- episodic + consolidation ----
    def add_turn(self, user, blur, person=None):
        self.episodes.append({"t": time.time(), "person": person or self.current_person,
                              "user": user, "blur": blur})
        if len(self.episodes) > self.max_episodes:
            self.episodes = self.episodes[-self.max_episodes:]   # bounded ring

    def consolidate(self):
        """Crystallise repeated facts, decay the irrelevant, stay bounded."""
        now = time.time()
        for p in self.people.values():
            for f in p["facts"]:
                if f.count >= 2:
                    f.confidence = min(0.99, f.confidence + 0.05)
            p["facts"].sort(key=lambda f: -f.salience(now))
            keep = [f for f in p["facts"] if f.salience(now) > 0.1][: self.max_facts]
            p["facts"] = keep

    # ---- persistence (the entity's state graph on 'flash') ----
    def save(self):
        if not self.path:
            return
        data = {"current_person": self.current_person,
                "episodes": self.episodes[-self.max_episodes:],
                "people": {n: {"facts": [asdict(f) for f in d["facts"]], "prefs": d["prefs"]}
                           for n, d in self.people.items()}}
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        json.dump(data, open(self.path, "w"), indent=1)

    def load(self):
        data = json.load(open(self.path))
        self.current_person = data.get("current_person", "friend")
        self.episodes = data.get("episodes", [])
        for n, d in data.get("people", {}).items():
            self.people[n] = {"facts": [Fact(**f) for f in d["facts"]], "prefs": d.get("prefs", {})}

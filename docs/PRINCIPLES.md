# Principles

Seven bedrock principles govern how runa makes runtime and boundary decisions. Architecture, contract enforcement, and contribution standards derive from these principles or are shown to conflict with them.

## Context

Autonomous AI agents are becoming dominant users of development infrastructure. runa builds runtime infrastructure for that world: contract enforcement, artifact tracking, and handoff mechanics that support methodology at scale. Plugins such as Groundwork layer domain methodology on top of that runtime boundary. These principles emerged from sustained practice building and operating agentic systems, not from theory.

## The Topology

The principles organize around four structural questions every methodology must answer, with a center that connects them:

- **North, the Physics**: how the system is constituted
  - **Sovereignty**
  - **Sequence**
- **East, the Stance**: how participants orient
  - **Grounding**
  - **Obligation to Dissent**
- **South, the Immune System**: how the system protects itself
  - **Recursive Improvement**
- **West, the Vector**: how work lands
  - **Transmission**
  - **Verifiable Completion**
- **Center, the Spiral**: the pipeline is the principles in motion

Remove any cardinal and you get a named failure:

- **No North:** sovereignty confusion and arbitrary sequencing. The system has no shape.
- **No East:** participants inherit assumptions and let problems pass. The system cannot see.
- **No South:** object-only work and entropy accumulation. The system decays.
- **No West:** work that never ships or ships without verification. The system produces but does not deliver.

## North: The Physics

### 1. Sovereignty

Clean boundaries between who owns what. The operator declares WHAT: direction, vision, intent. The agent owns HOW: execution, implementation, craft. HOW-ownership means full judgment and artistry, not mechanical compliance. The agent interprets, not transcribes.

Declarative framing is the communication discipline that expresses sovereignty: specify outcomes and constraints, not procedure. When boundaries hold, work flows. When they blur, corruption is symmetric. Domination by either side breaks the system.

The principle is fractal. It applies at every interface: human-agent, agent-agent, skill-skill, and stage-stage.

### 2. Sequence

Position carries meaning. The pipeline is ordered, not a menu. Each stage's output is the next stage's required input. Skipping a stage is not efficiency; it is a topological violation. The downstream stage receives malformed input and produces structurally valid but wrongly grounded output.

Interventions are placed where their corresponding failure modes occur. Grounding fires before design because inherited framing is the failure mode at that boundary. BDD fires before implementation because vague specification is the failure mode at that boundary. Each skill exists at a specific pipeline position because that position is where its absence causes damage.

## East: The Stance

### 3. Grounding

Orient to the real need. Every artifact and decision justifies itself against today's exigency, not yesterday's momentum. Existing code is evidence about one attempt to meet requirements; it is not the requirements themselves. The question is always: what must this enable, and for whom?

Grounding distinguishes descriptive truth, what exists, from normative truth, what is needed. For design work, normative truth is the starting point. Treating current behavior as the definition of what should happen is the most common grounding failure.

Grounding re-fires on every new generative act, not once at project start. The trigger is creation, not sequence position.

### 4. Obligation to Dissent

If you see something wrong, you act. Fix it or queue it. No rationalizing, no scoping away, no "it was preexisting." Silence in the face of a known problem is complicity with that problem.

The integrity of an unsupervised agentic system depends on every participant, human or agent, refusing to let problems pass. A code review that notices issues and declares them out of scope has failed. An agent that spots a defect and defers because it was not assigned to fix it has failed.

Dissent is not obstruction. It is the structural requirement that keeps the system honest.

## South: The Immune System

### 5. Recursive Improvement

Every operation refines both the object and the process that created it. Single-level work, improving only the object, accumulates entropy in the process. Dual-level work compounds improvement.

This principle operates on all the others. It is the system's self-cleaning mechanism. A pipeline that delivers code but never improves its own stages decays. A team that ships features but never refines how it ships decays.

The recursive spiral, refinement plus propulsion, is the topology that prevents stagnation. After completing any work, ask: did this address both the immediate thing and the process that produced it?

## West: The Vector

### 6. Transmission

Work completes when the recipient can act on it, not when the maker finishes creating it. The door must fit who enters. Compression is limited by the recipient's capacity to cross into understanding. Artifacts that only the makers can parse are walls, not doors.

Recipients include future agents loading your artifacts, contributors reading your documentation, and future instances of yourself crossing context windows. The gratitude test applies: would the recipient thank you for how you structured this? If not, transmission failed regardless of how correct the content is.

### 7. Verifiable Completion

Every unit of work has mechanically verifiable completion criteria: file gates, typed artifacts, BDD contracts. Completion is observable and pass/fail, not subjective and approximate.

Without verifiable completion, agents cannot self-organize, reviewers cannot evaluate, and the pipeline cannot enforce its own contracts. At scale, unverifiable completion produces a system where "done" is an assertion rather than a fact.

## Center: The Spiral

The pipeline is the principles in motion. Each principle contributes to the pipeline's shape, and the pipeline is how the principles compose.

Refinement plus propulsion: a circle that moves forward. The seven principles are not a checklist. They are a topology. They hold together or fail together.

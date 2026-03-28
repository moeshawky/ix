---
name: using-superpowers
description: Use when starting any conversation - establishes how to find and use skills, requiring Skill tool invocation before ANY response including clarifying questions. This skill helps discover relevant skills from the 270+ available skills in the library.
---

<EXTREMELY-IMPORTANT>
If you think there is even a 1% chance a skill might apply to what you are doing, you ABSOLUTELY MUST check for it.

IF A SKILL APPLIES TO YOUR TASK, YOU DO NOT HAVE A CHOICE. YOU MUST USE IT.

This is not negotiable. This is not optional. You cannot rationalize your way out of this.
</EXTREMELY-IMPORTANT>

## Capability Discovery

### Tier 1: Project Context
**CHECK FIRST BEFORE ANY ACTION**
- Read CLAUDE.md for project-specific instructions
- Check for existing automation scripts in the workspace
- **Rule:** Existing automation > new code

### Tier 2: Skill Library (270+ skills)
**CHECK SECOND FOR ENHANCEMENT PATTERNS**
- Process skills first (how to approach)
- Domain skills second (what to use)
- **Rule:** Skills enhance workflows, don't replace them

## Quick Skill Discovery

When you need to find a skill for a task, search these common categories:

### Development & Code
- **test-driven-development**: Writing tests before code
- **systematic-debugging**: Debugging any bug or issue
- **test-fixing**: Fixing failing tests
- **code-review-excellence**: Reviewing or requesting code reviews
- **receiving-code-review**: Handling code review feedback
- **frontend-dev-guidelines**: React/Next.js/TypeScript development
- **backend-dev-guidelines**: Node.js/Express/TypeScript backend
- **react-modernization**: Upgrading React versions

### Planning & Workflow
- **brainstorming**: Any creative work, building features, adding functionality
- **writing-plans**: Creating detailed execution plans
- **planning-with-files**: Complex multi-step task planning
- **executing-plans**: Executing existing implementation plans

### Data & Science
- **polars**: Fast DataFrame operations (Apache Arrow)
- **pandas**: Data analysis with pandas
- **scikit-learn**: Machine learning
- **matplotlib**: Scientific plotting
- **seaborn**: Statistical visualization
- **plotly**: Interactive visualizations

### Bioinformatics & Biology
- **biopython**: Molecular biology operations
- **scanpy**: Single-cell RNA-seq analysis
- **pydeseq2**: Differential gene expression
- **pymatgen**: Materials science
- **biomni**: Biomedical AI agent

### Chemistry & Drug Discovery
- **rdkit**: Molecular operations and fingerprints
- **datamol**: Simplified RDKit wrapper
- **pymol**: Molecular visualization
- **dockstring**: Molecular docking
- **diffdock**: Diffusion-based docking
- **deepchem**: Molecular ML

### Database APIs (270+ skills available)
- **pubchem-database**: Chemical compounds (110M+)
- **chembl-database**: Bioactivity data
- **uniprot-database**: Protein sequences
- **pdb-database**: Protein structures
- **alphafold-database**: AlphaFold predictions
- **ensembl-database**: Genome data
- **gene-database**: NCBI Gene
- **clinvar-database**: Clinical variants
- **gwas-database**: GWAS Catalog

### Documents & Office
- **pdf**: PDF manipulation
- **xlsx**: Excel spreadsheets
- **docx**: Word documents
- **pptx**: PowerPoint presentations

### Design & Creative
- **frontend-design**: Production-grade UI/UX
- **ui-ux-pro-max**: UI/UX design intelligence
- **canvas-design**: Visual art creation
- **algorithmic-art**: Generative art
- **d3-viz**: Custom D3.js visualizations

## How to Access Capabilities

### Step 1: Check Project Context
```bash
# Check for existing automation in the workspace
find . -maxdepth 3 -name "*.sh" -o -name "*.md" | grep -i "<task-keyword>"
```

### Step 2: Access Skills
**In Claude Code:** Use the `Skill` tool with the skill name. Example: `Skill(name="brainstorming")`

**Invoke format:**
```
Skill tool invocation:
- name: "skill-name"
```

**Never use Read tool on skill files** - always use the Skill tool to load them.

## The Discovery Process

### Step 0: Check Project Context FIRST
**BEFORE checking skills, check for existing automation:**
```bash
# Search for relevant scripts/docs in the workspace
find . -maxdepth 3 -type f \( -name "*.sh" -o -name "*.md" \) | xargs grep -l "<task-keyword>" 2>/dev/null
```

### Step 1: Check for Process Skills
These determine HOW to approach the task:
- **brainstorming** - Any creative/feature work
- **systematic-debugging** - Any bug fixing
- **test-driven-development** - Writing new features
- **skill-creator** - Creating new skills

### Step 2: Check for Domain Skills
Look for skills matching your domain:
- Database APIs for bioinformatics/chemistry
- Framework-specific skills (React, Next.js, etc.)
- Tool-specific skills (Docker, Terraform, etc.)

### Step 3: Check for Task Skills
Match the specific action:
- **xlsx/pdf/docx** - Document creation
- **plotly/matplotlib/seaborn** - Visualization
- **testing-patterns** - Test writing

## The Rule

**Invoke relevant or requested skills BEFORE any response or action.** Even a 1% chance a skill might apply means you should invoke it to check. If an invoked skill turns out to be wrong, you don't need to use it.

## Red Flags

These thoughts mean STOP—you're rationalizing:

| Thought | Reality |
|---------|---------|
| "This is just a simple question" | Questions are tasks. Check workflows AND skills. |
| "I need more context first" | Workflow/skill check comes BEFORE clarifying questions. |
| "Let me explore the codebase first" | Workflows/skills tell you HOW to explore. Check first. |
| "I can check git/files quickly" | Files lack conversation context. Check workflows/skills. |
| "Let me gather information first" | Workflows/skills tell you HOW to gather information. |
| "This doesn't need a formal skill/workflow" | If capability exists, use it. |
| "I remember this skill" | Skills/workflows evolve. Check current version. |
| "This doesn't count as a task" | Action = task. Check workflows AND skills. |
| "The skill/workflow is overkill" | Simple things become complex. Use it. |
| "I'll just do this one thing first" | Check workflows AND skills BEFORE doing anything. |
| "This feels productive" | Undisciplined action wastes time. Capabilities prevent this. |
| "I know what that means" | Knowing the concept ≠ using the capability. Check it. |
| "I'll write this from scratch" | Check /workflow/ folder for existing automation first. |
| "The workflow folder is just for cron jobs" | All automation belongs there - check it. |
| "Let me execute directly" | Run pre-hook.sh validation first. |

## Skill Priority

When multiple skills could apply, use this order:

1. **Process skills first** (brainstorming, debugging) - these determine HOW to approach the task
2. **Implementation skills second** (frontend-design, mcp-builder) - these guide execution

"Let's build X" → brainstorming first, then implementation skills.
"Fix this bug" → debugging first, then domain-specific skills.

## Skill Types

**Rigid** (TDD, debugging): Follow exactly. Don't adapt away discipline.

**Flexible** (patterns): Adapt principles to context.

The skill itself tells you which.

## User Instructions

Instructions say WHAT, not HOW. "Add X" or "Fix Y" doesn't mean skip skills.

## Skill Integration Protocol

### Mandatory Check Sequence:
1. **Project Context Check**
   - Search workspace for existing automation
   - Check CLAUDE.md for project-specific guidance
   - **Rule:** Existing automation > new code

2. **Skill Library Check** (270+ skills)
   - Process skills first (how to approach)
   - Domain skills second (what to use)
   - **Rule:** Skills enhance, don't replace existing patterns

### Example: Code Modification Task
```
1. Check workspace → Found existing test suite
2. Check CLAUDE.md → Found build/test commands
3. Check skills → test-driven-development, systematic-debugging
4. Execute with skill guidance
```

## Available Skill Categories

| Category | Example Skills |
|----------|---------------|
| **Bioinformatics** | biomni, biopython, scanpy, pydeseq2, anndata |
| **Chemistry** | rdkit, datamol, deepchem, diffdock |
| **Database APIs** | pubchem, chembl, uniprot, pdb, alphafold, ensembl |
| **ML/AI** | scikit-learn, transformers, pytorch-lightning, shap |
| **Visualization** | matplotlib, seaborn, plotly, claude-d3js-skill |
| **Development** | frontend-dev-guidelines, backend-dev-guidelines |
| **Testing** | test-driven-development, playwright-skill, testing-patterns |
| **Documents** | pdf-official, xlsx-official, docx-official, pptx-official |
| **Design** | frontend-design, ui-ux-pro-max, canvas-design |
| **Planning** | brainstorming, writing-plans, planning-with-files |
| **Security** | ethical-hacking-methodology, pentest-checklist, top-web-vulnerabilities |
| **Infrastructure** | k8s-manifest-generator, terraform-module-library, helm-chart-scaffolding |
| **API/Backend** | api-design-principles, fastapi-templates, stripe-integration |

## Finding Unknown Capabilities

When you encounter a domain you don't recognize:
1. **Check workspace first** for existing scripts/automation
2. **Check skills second** by searching the skill list in system context
3. **Use skill-creator** to build new skills if no match exists

## Success Metrics
- **Skill utilization**: % of tasks using applicable skills
- **Automation reuse**: % of tasks leveraging existing scripts
- **Duplicate work prevented**: Count of redundant implementations avoided

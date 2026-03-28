import os
import json
import re
import argparse
import sys

def extract_metadata(content):
    """Extracts name and description from SKILL.md content (Frontmatter or Markdown)."""
    name = None
    description = None

    # 1. Try Frontmatter (YAML)
    fm_match = re.search(r'^---\s*(.*?)\s*---', content, re.DOTALL)
    if fm_match:
        fm_content = fm_match.group(1)
        name_fm = re.search(r'^name:\s*(.+)$', fm_content, re.MULTILINE)
        desc_fm = re.search(r'^description:\s*(.+)$', fm_content, re.MULTILINE)
        
        if name_fm:
            name = name_fm.group(1).strip().strip('"\'')
        if desc_fm:
            description = desc_fm.group(1).strip().strip('"\'')

    # 2. Fallback to Markdown Header
    if not name:
        name_match = re.search(r'^#\s+(.+)$', content, re.MULTILINE)
        if name_match:
             name = name_match.group(1).strip()

    # 3. Fallback for Description (First text paragraph)
    if not description:
        body = content
        if fm_match:
            body = content[fm_match.end():].strip()
        
        lines = body.split('\n')
        desc_lines = []
        for line in lines:
            if line.startswith('#') or not line.strip():
                if desc_lines: break # Stop at next header or empty line after content
                continue
            desc_lines.append(line.strip())
        
        if desc_lines:
            description = " ".join(desc_lines)[:300]
            if len(description) == 300: description += "..."

    return name, description

def generate_index(skills_dir, output_file):
    print(f"🏗️  Scanning skills in: {skills_dir}")
    if not os.path.exists(skills_dir):
        print(f"❌ Error: Skills directory not found: {skills_dir}")
        return False

    skills = []
    # Calculate repo root relative to the output file
    repo_root = os.path.dirname(os.path.abspath(output_file))

    for root, dirs, files in os.walk(skills_dir):
        if "SKILL.md" in files:
            skill_path = os.path.join(root, "SKILL.md")
            dir_name = os.path.basename(root)
            
            # Calculate relative path for JSON
            try:
                rel_path = os.path.relpath(root, repo_root)
                rel_path = rel_path.replace(os.sep, '/')
            except ValueError:
                rel_path = dir_name

            name, description = None, None
            try:
                with open(skill_path, 'r', encoding='utf-8') as f:
                    content = f.read()
                name, description = extract_metadata(content)
            except Exception as e:
                print(f"⚠️  Error reading {skill_path}: {e}")

            if not name:
                name = dir_name.replace("-", " ").title()

            skills.append({
                "id": dir_name,
                "path": rel_path,
                "name": name,
                "description": description or ""
            })

    skills.sort(key=lambda x: x["name"])

    try:
        with open(output_file, 'w', encoding='utf-8') as f:
            json.dump(skills, f, indent=2)
        print(f"✅ Generated index with {len(skills)} skills.")
        print(f"📍 Location: {output_file}")
    except Exception as e:
        print(f"❌ Failed to write index: {e}")
        return False

    return True

def validate_skills(skills_dir):
    print(f"🔍 Validating skills in: {skills_dir}")
    errors = []
    count = 0

    for root, dirs, files in os.walk(skills_dir):
        if "SKILL.md" in files:
            count += 1
            path = os.path.join(root, "SKILL.md")
            rel = os.path.basename(root)
            
            try:
                with open(path, 'r', encoding='utf-8') as f:
                    content = f.read()
                
                name, desc = extract_metadata(content)
                
                if not name:
                    errors.append(f"❌ {rel}: Missing 'name'")
                if not desc:
                    errors.append(f"⚠️  {rel}: Missing 'description'")
                
            except Exception as e:
                errors.append(f"❌ {rel}: Read error {e}")

    if errors:
        print(f"\n⚠️  Found {len(errors)} issues in {count} skills:")
        for e in errors:
            print(e)
        return False
    
    print(f"✨ Validation Passed: {count} skills checked.")
    return True

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Generate index and validate Claude Skills.")
    parser.add_argument("--skills-dir", default="skills", help="Directory containing skill folders")
    parser.add_argument("--output", default="skills_index.json", help="Output JSON file path")
    parser.add_argument("--validate-only", action="store_true", help="Run validation only")
    
    args = parser.parse_args()
    
    # Determine absolute paths
    cwd = os.getcwd()
    script_dir = os.path.dirname(os.path.abspath(__file__))
    
    # Heuristic to find 'skills' folder:
    # 1. Look in CWD (current working directory)
    # 2. Look relative to script location
    # 3. Look in parent of script location (if script is in scripts/)
    
    candidates = [
        os.path.join(cwd, args.skills_dir),
        os.path.join(script_dir, args.skills_dir),
        os.path.join(os.path.dirname(script_dir), args.skills_dir)
    ]
    
    skills_path = None
    for c in candidates:
        if os.path.exists(c):
            skills_path = c
            break
            
    if not skills_path:
        # Fallback to default behavior even if missing, to let error handler catch it
        skills_path = os.path.abspath(args.skills_dir)
        print(f"⚠️  Could not auto-locate '{args.skills_dir}'. Using: {skills_path}")
    else:
        print(f"📂 Resolved skills directory: {skills_path}")

    # Output defaults to CWD unless specified otherwise
    output_path = os.path.abspath(args.output)

    if args.validate_only:
        validate_skills(skills_path)
    else:
        generate_index(skills_path, output_path)
        validate_skills(skills_path)

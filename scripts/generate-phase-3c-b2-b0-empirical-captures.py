#!/usr/bin/env python3
"""Generate isolated B2-B0 Git-output captures; never parses unified diff."""
import argparse
import hashlib
import json
import os
import platform
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "docs" / "fixtures" / "phase-3c-b2-b0"
TIMEOUT = 10
MAX_CHILD_STDOUT_BYTES = 2 * 1024 * 1024
MAX_CHILD_STDERR_BYTES = 16 * 1024
PREFIX = ["git", "-c", "core.quotePath=false", "-c", "color.ui=false", "-c", "core.fsmonitor=false", "diff"]
DIFF_ARGS = ["--no-ext-diff", "--no-textconv", "--no-color", "--no-renames", "--full-index", "--src-prefix=a/", "--dst-prefix=b/"]

SCENARIOS = [
    ("E01-standard", "unstaged", b"file.txt", b"keep\nold\n", b"keep\nnew\n"),
    ("E02-staged-addition", "staged", b"new.txt", None, b"added\n"),
    ("E03-staged-deletion", "staged", b"old.txt", b"deleted\n", None),
    ("E04-crlf", "unstaged", b"crlf.txt", b"old\r\nkeep\r\n", b"new\r\nkeep\r\n"),
    ("E05-old-no-final", "unstaged", b"old-none.txt", b"old", b"new\n"),
    ("E06-new-no-final", "unstaged", b"new-none.txt", b"old\n", b"new"),
    ("E07-both-no-final", "unstaged", b"both-none.txt", b"old", b"new"),
    ("E08-context-both-none", "unstaged", b"context.txt", b"old\nfinal", b"new\nfinal"),
    ("E09-one-space", "unstaged", b"one space.txt", b"old\n", b"new\n"),
    ("E10-multi-space", "unstaged", b"many  spaces.txt", b"old\n", b"new\n"),
    ("E11-leading-dash", "unstaged", b"-dash.txt", b"old\n", b"new\n"),
    ("E12-unicode-bmp", "unstaged", "bmp-π.txt".encode(), b"old\n", b"new\n"),
    ("E13-unicode-supplementary", "unstaged", "supp-😀.txt".encode(), b"old\n", b"new\n"),
    ("E14-unicode-composed", "unstaged", "composed-é.txt".encode(), b"old\n", b"new\n"),
    ("E15-unicode-decomposed", "unstaged", "decomposed-e\u0301.txt".encode(), b"old\n", b"new\n"),
    ("E16-backslash", "unstaged", b"back\\slash.txt", b"old\n", b"new\n"),
    ("E17-double-quote", "unstaged", b'double"quote.txt', b"old\n", b"new\n"),
]

CLAIMS = {
    "E01-standard": ["ordinary ASCII path", "LF source lines", "controlled context/deletion/addition source change"],
    "E02-staged-addition": ["pre-image absent", "post-image staged"],
    "E03-staged-deletion": ["pre-image committed", "post-image absent from staged index"],
    "E04-crlf": ["before and after source lines use CRLF"],
    "E05-old-no-final": ["old source lacks final newline", "new source has final newline"],
    "E06-new-no-final": ["old source has final newline", "new source lacks final newline"],
    "E07-both-no-final": ["both changed source sides lack final newlines"],
    "E08-context-both-none": ["nearby line changes", "unchanged final source line lacks final newline on both sides"],
    "E09-one-space": ["requested path contains one space"],
    "E10-multi-space": ["requested path contains multiple spaces"],
    "E11-leading-dash": ["requested path begins with dash and follows --"],
    "E12-unicode-bmp": ["requested path contains BMP UTF-8 bytes"],
    "E13-unicode-supplementary": ["requested path contains supplementary-plane UTF-8 bytes"],
    "E14-unicode-composed": ["requested path uses composed Unicode bytes"],
    "E15-unicode-decomposed": ["requested path uses decomposed Unicode bytes"],
    "E16-backslash": ["requested path contains a literal backslash byte"],
    "E17-double-quote": ["requested path contains a literal double-quote byte"],
}

CONDITIONAL_SCENARIOS = [
    ("C01-leading-space", "policy-rejected", "current B2-B0 bridge-safe path policy has not authorized leading-space paths"),
    ("C02-trailing-space", "policy-rejected", "current B2-B0 bridge-safe path policy has not authorized trailing-space paths"),
    ("C03-literal-tab", "policy-rejected", "literal TAB paths are outside the approved path policy"),
    ("C04-staged-unusual-addition", "policy-rejected", "requires later approval of an unusual-path staged-addition policy"),
    ("C05-staged-unusual-deletion", "policy-rejected", "requires later approval of an unusual-path staged-deletion policy"),
]

def fail(message):
    raise SystemExit(message)

def has_symlink_component(path):
    current = path.expanduser()
    parts = current.parts
    probe = Path(parts[0]) if current.is_absolute() else Path()
    for part in parts[1:] if current.is_absolute() else parts:
        probe /= part
        if probe.is_symlink():
            return True
    return False

def safe_output(value):
    requested = Path(value).expanduser()
    if has_symlink_component(requested):
        fail("refusing symlink output path")
    out = requested.resolve(strict=False)
    if out == ROOT or ROOT in out.parents or out == FIXTURES or FIXTURES in out.parents:
        fail("refusing repository output path")
    if out.exists() and (out.is_symlink() or not out.is_dir() or any(out.iterdir())):
        fail("output directory must be a fresh empty non-symlink directory")
    for ancestor in (out, *out.parents):
        if ancestor.exists() and (ancestor / ".git").exists():
            fail("refusing output inside an existing Git repository")
    if not out.exists(): out.mkdir(parents=True)
    if (out / ".git").exists(): fail("refusing existing Git repository output")
    return out

def run(argv, cwd, env):
    result = subprocess.run(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=TIMEOUT, check=False)
    if result.returncode != 0: fail("isolated Git command failed")
    return result

def file_state(path, data, present, staged, worktree):
    endings = None if data is None else ("CRLF" if b"\r\n" in data else "LF")
    return {"path_bytes_hex": path.hex(), "path_byte_length": len(path), "path_utf8": path.decode("utf-8", "strict"), "present": present, "mode": "100644" if present else None, "content_bytes_hex": data.hex() if data is not None else None, "content_byte_length": len(data) if data is not None else 0, "index_presence": staged, "worktree_presence": worktree, "line_ending_layout": endings, "final_newline_present": bool(data and data.endswith((b"\n", b"\r\n")))}

def write_exclusive(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "xb") as handle: handle.write(data)

def require_filename_bytes(repo, rel):
    wanted = os.fsdecode(rel)
    actual = {os.fsencode(entry.name) for entry in os.scandir(repo)}
    if rel not in actual:
        fail("filesystem did not preserve requested path bytes")
    if os.fsencode(wanted) != rel:
        fail("filesystem path decoding changed requested path bytes")

def capture(out, spec, git_version):
    cid, surface, rel, before, after = spec
    base = out / "repositories" / cid
    repo = base / "repo"; home = base / "isolated-home"; repo.mkdir(parents=True); home.mkdir()
    global_cfg = base / "isolated-global.gitconfig"; system_cfg = base / "isolated-system.gitconfig"; global_cfg.write_bytes(b""); system_cfg.write_bytes(b"")
    env = {"LC_ALL":"C","LANG":"C","GIT_CONFIG_NOSYSTEM":"1","GIT_CONFIG_GLOBAL":str(global_cfg),"GIT_CONFIG_SYSTEM":str(system_cfg),"HOME":str(home),"XDG_CONFIG_HOME":str(home / "xdg"),"GIT_PAGER":"cat","PAGER":"cat","GIT_TERMINAL_PROMPT":"0","GIT_AUTHOR_DATE":"2000-01-01T00:00:00+0000","GIT_COMMITTER_DATE":"2000-01-01T00:00:00+0000"}
    env.update({k:v for k,v in os.environ.items() if k in ("PATH","SystemRoot")})
    commands=[]
    metadata_env={"LC_ALL":"C","LANG":"C","GIT_CONFIG_NOSYSTEM":"1","GIT_CONFIG_GLOBAL":"isolated-global.gitconfig","GIT_CONFIG_SYSTEM":"isolated-system.gitconfig","HOME":"isolated-home","XDG_CONFIG_HOME":"isolated-home/xdg","GIT_PAGER":"cat","PAGER":"cat","GIT_TERMINAL_PROMPT":"0","GIT_AUTHOR_DATE":"2000-01-01T00:00:00+0000","GIT_COMMITTER_DATE":"2000-01-01T00:00:00+0000"}
    def step(argv): commands.append({"argv":argv,"cwd":"repo","environment_overrides":dict(metadata_env),"expected_exit_status":0}); return run(argv, repo, env)
    step(["git","init","-q"])
    env["GIT_WORK_TREE"] = str(repo)
    env["GIT_DIR"] = str(repo / ".git")
    metadata_env["GIT_WORK_TREE"] = "repo"
    metadata_env["GIT_DIR"] = "repo/.git"
    step(["git","config","user.name","B2 B0"]); step(["git","config","user.email","b2b0@example.invalid"])
    target = os.fsdecode(rel); path = repo / target
    if before is not None:
        path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(before)
        require_filename_bytes(repo, rel)
        step(["git","add","--",target])
    else:
        anchor = repo / ".b2b0-anchor"; anchor.write_bytes(b"anchor\n"); step(["git","add","--",".b2b0-anchor"])
    step(["git","commit","-qm","baseline"])
    if after is None: path.unlink()
    else:
        path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(after); require_filename_bytes(repo, rel)
    if surface == "staged": step(["git","add","-A","--",target])
    argv = PREFIX + (["--cached"] if surface == "staged" else []) + DIFF_ARGS + ["--",target]
    result = run(argv, repo, env)
    if result.stderr:
        fail("isolated Git command wrote stderr")
    if len(result.stdout) > MAX_CHILD_STDOUT_BYTES or len(result.stderr) > MAX_CHILD_STDERR_BYTES:
        fail("isolated Git output exceeded B2 child cap")
    captures = out / "captures"; captures.mkdir(exist_ok=True)
    stdout_rel=f"captures/{cid}.stdout.bin"; stderr_rel=f"captures/{cid}.stderr.bin"; write_exclusive(out / stdout_rel,result.stdout); write_exclusive(out / stderr_rel,result.stderr)
    before_index = before is not None
    after_index = after is not None
    record={"schema_version":1,"capture_id":cid,"scenario":cid.removeprefix("E"),"surface":surface,"git_version":git_version,"operating_system":platform.platform(),"repository_relative_path":f"repositories/{cid}/repo","temporary_root_creation_policy":"explicit fresh output directory","environment":metadata_env,"git_config":{"core.quotePath":"false","color.ui":"false","core.fsmonitor":"false","external_diff":"disabled by --no-ext-diff","textconv":"disabled by --no-textconv","renames_and_copies":"disabled by --no-renames","mnemonic_prefix":"false by default fixed prefixes","relative_output":"false by fixed prefixes","src_prefix":"a/","dst_prefix":"b/"},"setup_commands":commands,"before_state":[file_state(rel,before,before is not None,before_index,before is not None)],"after_state":[file_state(rel,after,after is not None,after_index,after is not None)],"capture_command":{"argv":argv,"cwd":"repo","environment_overrides":metadata_env,"expected_exit_status":0},"stdout_relative_path":stdout_rel,"stdout_sha256":hashlib.sha256(result.stdout).hexdigest(),"stdout_byte_length":len(result.stdout),"stderr_relative_path":stderr_rel,"stderr_sha256":hashlib.sha256(result.stderr).hexdigest(),"stderr_byte_length":len(result.stderr),"source_claims_for_manual_review":CLAIMS[cid] + ["stdout remains opaque; this tool performs no diff interpretation"],"no_production_repository_used":True,"no_user_worktree_used":True,"no_product_agent_used":True,"notes":"fixture-only isolated capture"}
    return record

def main():
    p=argparse.ArgumentParser(); p.add_argument("--output-dir"); p.add_argument("--scenario",default="all"); p.add_argument("--list-scenarios",action="store_true"); p.add_argument("--compare-to") ; args=p.parse_args()
    if args.list_scenarios:
        for cid,surface,*_ in SCENARIOS: print(f"{cid}\t{surface}\trequired")
        for cid, disposition, precondition in CONDITIONAL_SCENARIOS:
            print(f"{cid}\tconditional\t{disposition}\t{precondition}")
        return
    if not args.output_dir: fail("--output-dir is required")
    selected=SCENARIOS if args.scenario=="all" else [s for s in SCENARIOS if s[0]==args.scenario]
    if not selected: fail("unknown scenario")
    out=safe_output(args.output_dir); git_version=subprocess.check_output(["git","--version"],text=True).strip(); records=[capture(out,s,git_version) for s in selected]
    write_exclusive(out/"empirical-captures.jsonl",("\n".join(json.dumps(x,sort_keys=True,separators=(",",":")) for x in records)+"\n").encode())
    lines=["capture_id\tscenario\tsurface\tstdout_relative_path\tstdout_sha256\tstdout_byte_length\tstderr_relative_path\tstderr_sha256\tstderr_byte_length\trepository_relative_path\tprimary_path_bytes_hex\tgit_version"]
    for r in records: lines.append("\t".join([r["capture_id"],r["scenario"],r["surface"],r["stdout_relative_path"],r["stdout_sha256"],str(r["stdout_byte_length"]),r["stderr_relative_path"],r["stderr_sha256"],str(r["stderr_byte_length"]),r["repository_relative_path"],r["before_state"][0]["path_bytes_hex"],r["git_version"]]))
    write_exclusive(out/"capture-manifest.tsv",("\n".join(lines)+"\n").encode())
    if args.compare_to:
        other=Path(args.compare_to)
        for relpath in ["empirical-captures.jsonl","capture-manifest.tsv"]+[r["stdout_relative_path"] for r in records]+[r["stderr_relative_path"] for r in records]:
            if (out/relpath).read_bytes() != (other/relpath).read_bytes(): fail("comparison mismatch")
    print(f"generated {len(records)} isolated empirical captures")
if __name__=="__main__": main()

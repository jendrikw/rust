#!/usr/bin/env python3

import argparse
import fnmatch
import subprocess as sp
import sys
from typing import Any

try:
    import tomllib
except ImportError:
    raise Exception("this script requires Python 3.11+ (relies on tomllib)")

DEFAULT_BRANCH: str = "master"
TARGETS: list[str] = [
    "library/std",
    "library/core",
    "library/alloc",
]


def eprint(*args, **kwargs):
    """Print to stderr."""
    print(*args, file=sys.stderr, **kwargs)


def parse_args() -> argparse.Namespace:
    """Define and parse CLI arguments."""
    parser = argparse.ArgumentParser(
        prog="Rust get-maintainers",
        description="Find maintainers, teams, and CLI commands based on changed files",
    )
    parser.add_argument(
        "-f",
        "--file-name",
        type=argparse.FileType("rb"),
        help="Path to triagebot.toml-style configuration",
        default="triagebot.toml",
    )
    parser.add_argument(
        "--to",
        type=str,
        help="Compare to this destination rev",
        default=DEFAULT_BRANCH,
    )
    parser.add_argument(
        "--from",
        type=str,
        help="Compare from this source rev",
        # default="HEAD",
        dest="from_",
    )
    parser.add_argument(
        "-m",
        "--maintainers",
        help="List maintainers for given changes",
        action="store_true",
        default=True,
    )
    return parser.parse_args()


def root_dir() -> bytes:
    return sp.check_output(["git", "rev-parse", "--show-toplevel"], stderr=sp.STDOUT)


def changed_files_git(args) -> list[bytes]:
    """Use git to find changed files."""
    try:
        to_rev = (
            sp.check_output(["git", "rev-parse", args.to], stderr=sp.STDOUT)
            .decode()
            .strip()
        )

        if args.from_ is not None:
            from_rev = (
                sp.check_output(["git", "rev-parse", args.from_], stderr=sp.STDOUT)
                .decode()
                .strip()
            )
            diffparam = f"{to_rev}..{from_rev}"
        else:
            diffparam = to_rev

        fnames = sp.check_output(
            ["git", "diff", "--name-only", diffparam], stderr=sp.STDOUT
        )
    except sp.CalledProcessError:
        eprint("Failed to process revisions")
        sys.exit(1)

    # prepend absolute path for glob matching
    ret = [b"/" + n for n in fnames.strip().split(b"\n")]

    if len(ret) == 0:
        eprint("No changes found")

    return ret


OwnerMap = dict[str, list[str]]
GroupMap = dict[str, list[str]]


def find_owners(fnames: list[bytes], owner_map: OwnerMap) -> set[str]:
    """Given a list of files, match all possible owners"""
    ret: set[str] = set()

    for fname in fnames:
        for pat, owners in owner_map.items():
            bpat = pat.encode()
            if fnmatch.fnmatch(fname, bpat) or fnmatch.fnmatch(fname, bpat + b"/*"):
                ret.update(owners)

    return ret


def find_targets(fnames: list[bytes]) -> set[str]:
    """Given a list of files, indicate which build/test targets should be run"""


def print_owners(owners: set[str], groups: GroupMap):
    """Expand and prettyprint owner teams and users"""
    users = [u for u in owners if u.startswith("@")]
    teams = [t for t in owners if not t.startswith("@")]

    if len(users) > 0:
        print("Users:")
    for user in sorted(users):
        print(f"    {user}")

    if len(teams) > 0:
        print("Teams:")

    for team in sorted(teams):
        print(f"    {team}")
        members = groups[team]
        users = [u for u in members if u.startswith("@")]
        subteams = [s for s in members if not s.startswith("@")]

        for subt in sorted(subteams):
            print(f"        {subt} (subteam)")
            for member in sorted(groups[subt]):
                print(f"            {member}")

        for user in sorted(users):
            print(f"        {user}")


def load_toml(args: argparse.Namespace) -> tuple[GroupMap, OwnerMap]:
    data = tomllib.load(args.file_name)

    assign: dict[str, Any] = data["assign"]
    group_map: GroupMap = assign["adhoc_groups"]
    owner_map: OwnerMap = assign["owners"]

    return (group_map, owner_map)


def main():
    args = parse_args()
    (group_map, owner_map) = load_toml(args)
    fnames = changed_files_git(args)
    owners = find_owners(fnames, owner_map)
    print_owners(owners, group_map)


if __name__ == "__main__":
    main()

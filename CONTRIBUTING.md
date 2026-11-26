# Contributing

This text applies to each repository that carries it. Each repository takes its own contributions.

## Licence of contributions

Contributions are accepted under the repository's MIT licence. You keep your copyright. There is no contributor licence agreement.

## Sign-off (required)

Every commit must carry a sign-off line:

    Signed-off-by: Your Name or Handle <email>

Add it with `git commit -s`. Pull requests with unsigned commits are not merged. By signing off you certify the Developer Certificate of Origin 1.1 (https://developercertificate.org) and, in addition, everything in the next section.

## What you certify

By signing off, you certify that your contribution:

- Is your own work, or you have the right to submit it under the MIT licence.
- Contains no game files, game assets, decompiled or disassembled listings, debug symbols, or encryption keys, in any form, including in comments, tests, screenshots and commit messages.
- Was not taken from, and was not produced by working from, leaked, stolen or confidential material. That includes source code, internal or pre-release builds, internal tools and internal documents.
- Was not copied or translated from a project under an incompatible licence (for example GPL code into this MIT project).
- Does not add anything aimed at online play, at defeating a copy protection, licence check or anti-cheat measure, or at obtaining, decrypting or unpacking a game.

This repository takes original code only. It does not accept data about any game: no addresses, structure layouts, name lists, hashes or signature patterns. Pull requests that add them are closed.

## If a contribution breaks these rules

Report it privately through GitHub's "Report a vulnerability" button on the repository's Security tab. Do not open a public issue and do not quote the material.

Maintainers will:

- Close the pull request unmerged, or revert the change at once if it was merged.
- Remove the material from the full history (`git filter-repo`), force-push every affected branch and tag, and delete and rebuild any release that contained it.
- Ask GitHub Support to delete the pull request and purge cached and unreachable commits, which a force-push alone does not remove.
- Tell known fork owners to do the same. Forks are outside the maintainers' control.
- Record what was removed and when, without reproducing it.
- Refuse further contributions from the account if the breach looks deliberate.

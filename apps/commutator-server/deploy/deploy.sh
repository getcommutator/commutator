#!/bin/sh
# Ship the source to kuzyo and rebuild commutator-server there, in the pets
# container (kuzyo/README.md and pets/commutator/ in the infra repo).
set -e
cd "$(git rev-parse --show-toplevel)"
rsync -az --delete --exclude=/.git --exclude=/target --filter=':- .gitignore' ./ kuzyo:/srv/pets/commutator/src/
ssh kuzyo 'incus exec pets -- docker compose --project-directory /srv/pets/commutator up -d --build'

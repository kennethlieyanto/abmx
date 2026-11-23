# abmx config chart

A tiny Helm chart whose only job is to turn plain files into the two ConfigMaps
the `abmx` chart consumes:

- `abmx-config` — from `files/config.toml`
- `abmx-parsers` — one key per `files/parsers/<id>/parser.rhai`

## Use

Copy this directory into your private config repository, then edit:

```
files/config.toml
files/parsers/<id>/parser.rhai
files/parsers/<id>/fixtures/*.eml
```

`templates/configmaps.yaml` globs the parser directories, so adding a parser is
just a new directory — no manifest list to maintain.

Point the `abmx` chart at the generated ConfigMaps:

```yaml
config:
  existingConfigMap: abmx-config
parsers:
  existingConfigMap: abmx-parsers
  mode: replace   # the files above are the only corpus
```

Then let ArgoCD apply both charts. A push updates the ConfigMaps; the next
CronJob run uses them. No image rebuild and no chart version bump.

Keep this repository private: `config.toml` contains account UUIDs, and parser
fixtures contain real bank email.

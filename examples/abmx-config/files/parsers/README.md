# Parser corpus for the cluster.
#
# One directory per parser id:
#
#   files/parsers/<id>/config.toml
#   files/parsers/<id>/fixtures/*.eml
#
# `templates/configmaps.yaml` globs files/parsers/*/config.toml, so adding a
# directory is all that is needed. Fixtures stay in Git for local
# `abmx parser check`; they are not put in the ConfigMap.

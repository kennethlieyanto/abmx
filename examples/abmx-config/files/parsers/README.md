# Parser corpus for the cluster.
#
# One directory per parser id:
#
#   files/parsers/<id>/parser.rhai
#   files/parsers/<id>/fixtures/*.eml
#
# `templates/configmaps.yaml` globs files/parsers/*/parser.rhai, so adding a
# directory is all that is needed. Fixtures stay in Git for local
# `abmx parser check`; they are not put in the ConfigMap.

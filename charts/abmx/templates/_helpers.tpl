{{/*
Common helpers.
*/}}

{{- define "abmx.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "abmx.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{- define "abmx.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "abmx.labels" -}}
helm.sh/chart: {{ include "abmx.chart" . }}
{{ include "abmx.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{- define "abmx.selectorLabels" -}}
app.kubernetes.io/name: {{ include "abmx.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "abmx.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "abmx.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{- define "abmx.image" -}}
{{- if .Values.image.digest -}}
{{- printf "%s@%s" .Values.image.repository .Values.image.digest -}}
{{- else -}}
{{- printf "%s:%s" .Values.image.repository (default .Chart.AppVersion .Values.image.tag) -}}
{{- end -}}
{{- end -}}

{{- define "abmx.configMapName" -}}
{{- if .Values.config.existingConfigMap -}}
{{- .Values.config.existingConfigMap -}}
{{- else if .Values.config.data -}}
{{- printf "%s-config" (include "abmx.fullname" .) -}}
{{- else -}}
{{- fail "set config.existingConfigMap or config.data; the chart does not guess [labels], account mappings, or the Actual server" -}}
{{- end -}}
{{- end -}}

{{- define "abmx.pvcName" -}}
{{- if .Values.persistence.existingClaim -}}
{{- .Values.persistence.existingClaim -}}
{{- else -}}
{{- include "abmx.fullname" . -}}
{{- end -}}
{{- end -}}

{{- define "abmx.parserPaths" -}}
{{- if not (has .Values.parsers.mode (list "overlay" "replace")) -}}
{{- fail "parsers.mode must be 'overlay' or 'replace'" -}}
{{- end -}}
{{- if eq .Values.parsers.mode "overlay" -}}
{{- printf "%s:/usr/share/abmx/parsers" .Values.parsers.mountPath -}}
{{- else -}}
{{- .Values.parsers.mountPath -}}
{{- end -}}
{{- end -}}

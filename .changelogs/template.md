# CHANGELOG

<!-- generated from cargo-changelog -->

{{#if this.versions}}
{{#each (reverse (sort_versions this.versions))}}
## v{{this.version}}

{{#each (group_by_header this.entries "type" default="Misc")}}
### {{ @key }}

{{#each this ~}}
* {{this.header.subject}}
{{indent this.text spaces=2}}
{{/each ~}}
{{~ /each ~}}
{{~ /each ~}}
{{/if}}
{{#if this.suffix}}
{{this.suffix}}
{{/if}}

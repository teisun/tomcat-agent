# Tomcat packages

A package may contain a standalone `SKILL.md`, a standalone `plugin.json`, or a `package.json` whose `tomcat.skills` and `tomcat.plugins` point to directories inside the package. Confirm the intended scope, inspect the manifest, then use `package_install`; never manually copy a package into a managed directory.

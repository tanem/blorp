# Security

## Reporting a problem

Report a security problem privately through GitHub's private vulnerability reporting: on the repository's Security tab, choose "Report a vulnerability". Please do not open a public issue for it.

## What blorp stores

blorp stores one secret, the Fastmail API token, in the platform keyring under service `blorp` and user `fastmail`: the login keychain on macOS, Secret Service on Linux and other Unix systems, and Credential Manager on Windows. Release builds never write the token to a plain file. Where no keyring is available, the token comes from `BLORP_API_TOKEN` and is not stored.

In the code, the token is exposed only to build the `Authorization` header of requests to Fastmail and to write it to the token store. It is not logged.

## What the log holds

The log is described in the README's [Files and environment variables](README.md#files-and-environment-variables). It is emptied at each launch. By default it records warnings and errors. When Fastmail answers with an error that retrying would not fix, or with a response blorp cannot read, the log records the response body without its headers; that body can include mail content such as senders and subjects. `BLORP_LOG` can turn on more detail, such as push stream and retry events.

## Revoking a token

`blorp logout` deletes the stored token but does not revoke it. To revoke a token, delete it in Fastmail under Settings, Privacy & Security, Connected apps & API tokens. blorp then exits with a message saying the token was rejected, and `blorp setup` stores a new one.

## Dependency updates

Dependabot alerts are on for this repository. Renovate (`renovate.json`) opens the pull requests that update dependencies, including those the alerts name. Dependabot security updates are off, so Dependabot does not open pull requests of its own.

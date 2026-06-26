# PuppyNet

## Install

Install the latest Linux or macOS release for the current user and register PuppyNet as a user service:

```sh
curl -fsSL https://raw.githubusercontent.com/J45k4/puppynet/master/scripts/install.sh | sh
```

Install as a system service:

```sh
curl -fsSL https://raw.githubusercontent.com/J45k4/puppynet/master/scripts/install.sh | sh -s -- --system
```

Install only the binary:

```sh
curl -fsSL https://raw.githubusercontent.com/J45k4/puppynet/master/scripts/install.sh | sh -s -- --no-service
```

Install a specific release:

```sh
curl -fsSL https://raw.githubusercontent.com/J45k4/puppynet/master/scripts/install.sh | sh -s -- --version 5
```

Manage the installed service:

```sh
puppynet status
puppynet start
puppynet stop
puppynet restart
puppynet uninstall
```

Add `--system` to manage a system service instead of the current user's service.

## Run

Start the PuppyNet daemon:

```sh
puppynet daemon
```

Running `puppynet` without a subcommand also starts the daemon for compatibility.

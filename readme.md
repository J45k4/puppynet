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

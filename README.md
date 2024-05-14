# 0721

![clippy workflow](https://img.shields.io/github/actions/workflow/status/langyo/0721/publish.yml)
![GitHub License](https://img.shields.io/github/license/langyo/0721)
![GitHub Tag](https://img.shields.io/github/v/tag/langyo/0721)

> Ciallo～(∠·ω< )⌒★

A simple engine for image hosting written in Rust.

## Features

- Image hosting
- Automatic image compression via WebP
- Basic account system
- Internationalization
- Deploy via Docker
- Written in pure Rust

## Deploy

```bash
docker run -d -p 8080:80 -v image-cache:/home/cache ghcr.io/langyo/0721:latest
```

## Configuration

> You can configure the engine via `Config.toml` in the docker volume.
>
> If you want to change the configuration, you might need to enter the container via the other image:
>
> ```bash
> docker run -it --rm -v image-cache:/home busybox:latest
> ```
>
> Then you can edit the `Config.toml` in the `/home` directory by using `vi`.
>
> After that, you can restart the container to apply the new configuration:
>
> ```bash
> docker restart <container_id>
> ```

```toml
[portal]
# The site's title
title-suffix = "Ciallo～(∠·ω< )⌒★"
# The banner in the footer
footer-banner = [
    { text = "© 0721 project", url = "https://github.com/langyo/0721" },
]
# The default language of the site
language = "zh_hans"
# The timezone of the server, it will affect the images' timeline page
timezone = +8

[router]
# The entry path of the images
media-entry-path = "/media"
# If it's empty, it means no limit
limit-referrer-host = []

[upload]
# Allow to use "KiB", "MiB" as unit, or pure number string as bytes
image-size-limit = "8MiB"
# If it's true, the uploaded image will be converted to WebP format automatically
webp-auto-convert = true
# If it's true, the uploaded image will keep the original file's name
use-source-file-name = false
```

> JWT secret key can be configured via the environment variable `JWT_SECRET`.
>
> You can configure it via the additional command line argument `-e JWT_SECRET=your_secret_key` for the `docker run` command.
>
> The default user is `admin` and the password is `admin` too. Modification of username and password is currently not supported. If operations are required, please create new users and delete old users. Subsequent versions will improve this capability.

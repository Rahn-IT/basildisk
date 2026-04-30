# Basildisk

## ⚠️WARNING⚠️

> [!WARNING]
> **This repository is heavily vibe coded.**
>
> While I built the template and database scheme myself, a lot of the code was built using codex.
>
> It's a very simple app and we use it ourselves, but I still feel like it should be openly disclosed in my opinion
> it will always influence code quality.
>
> Feel free to check out the code if you're unsure

Basildisk is a webinterface which allows checking and securely erasing hard drives and SSDs.

It's meant to be either run using a usb-stick on the machine you want to delete or to be installed on a machine with a bunch of drive sleds which you then chuck in a corner in your workshop.

## requirements

- lsblk
- smartctl
- hdparm
- nvme
- shred

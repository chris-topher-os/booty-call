# goal

a small utility installed on one control machine and another multi-booted target box on the same tailnet. lets me remotely boot up any of the target box's OS partitions, whether the target box is offline or already booted into a different OS.

# desired interface

- generic/easy cross OS install for non-control-plane switch nodes, working across at least Windows and Debian but should be easily extendable to other systems
- switch nodes let control node know they're alive, and can be told to shut down and boot into a different OS
- control plane hosts an api for registering switch nodes, querying which node is online, and switching to different nodes
- control plane also hosts a simple pwa frontend with a single ui control, a radio button group with one button for each switch node. the currently active node's button is disabled, pressing a button reboots the target box to that node, buttons show the various states of each node (offline, switching, online), and all buttons should be disabled while a switch is happening

# implementation thoughts

- switch nodes should follow the principle of least access and expose as little functionality as possible to implement what the control node needs from them. at minimum they need an authenticated command to reboot; liveness can come from tailscale rather than a custom heartbeat
- rebooting is slow, so the frontend will need to be designed around that
- rely on uefi bootnext since my target box firmware supports it
- booting from offline will likely require wake on lan, since i don't have a bmc on the consumer gaming pc that is my target box; and my control node will be on the same lan as the target box for the foreseeable future. ideally, though, the design shouldn't make moving the control node elsewhere too onerous
- booting from offline will likely be a worst-case two-step process: first send a "probe" wol packet that boots to whatever is the default, and if it isn't the right os, reboot to the desired one. my box will almost always be online anyway, so i don't mind treating this as a second-class workflow
- i only need to support a single target box and don't need features for additional boxes; but the design shouldn't make adding another box too onerous either

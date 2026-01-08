# Katana Hypervisor

A hypervisor for managing [Katana](https://github.com/dojoengine/dojo) instances with QEMU/KVM virtualization and optional AMD SEV-SNP TEE support.

## Overview

Katana Hypervisor provides VM-based isolation for Katana (Starknet sequencer) instances with:

- **Hardware-level isolation** using QEMU/KVM virtual machines
- **Resource management** (CPU, memory, storage quotas per instance)
- **Optional TEE support** with AMD SEV-SNP for confidential computing
- **Attestation verification** for cryptographic proof of execution environment
- **Port management** with automatic allocation
- **State persistence** across hypervisor restarts

## Features

### ✅ Phase 1: Core VM Management (Complete)

- Create, start, stop, delete instances
- List all instances with status
- View serial console logs
- Resource limits (vCPUs, memory, storage)
- Automatic port allocation (starting from 5050)
- SQLite state database
- QMP integration for VM control

### ✅ Phase 2: TEE Support (Complete)

- AMD SEV-SNP configuration
- Launch measurement calculation
- Remote attestation verification
- Reproducible builds for measurement consistency

### 🚧 Phase 3: Monitoring (Planned)

- Real-time resource statistics
- Health checks
- Storage quota enforcement
- Log streaming

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                  Katana Hypervisor                      │
│                     (Rust CLI)                          │
├─────────────────────────────────────────────────────────┤
│  State DB  │  Port Allocator  │  Storage Manager       │
└────────────┴──────────────────┴─────────────────────────┘
                        │
                        ▼
          ┌─────────────────────────────┐
          │      QEMU/KVM Manager       │
          │   (QMP for control)         │
          └─────────────────────────────┘
                        │
        ┌───────────────┼───────────────┐
        ▼               ▼               ▼
   ┌────────┐      ┌────────┐      ┌────────┐
   │ VM 1   │      │ VM 2   │      │ VM N   │
   │        │      │        │      │        │
   │ Katana │      │ Katana │      │ Katana │
   │ :5050  │      │ :5051  │      │ :505N  │
   └────────┘      └────────┘      └────────┘
```

## Prerequisites

### System Requirements

- **Linux** (Ubuntu 24.04, RHEL 9, Fedora 39+)
- **KVM support** (`lsmod | grep kvm`)
- **QEMU** 7.0+ with KVM (`qemu-system-x86_64`)
- **Rust** 1.75+

### Optional (for TEE mode)

- **AMD EPYC** 3rd gen (Milan) or newer with SEV-SNP
- **SEV-SNP enabled** in BIOS
- `/dev/sev-guest` device available

### Install Dependencies

#### Ubuntu/Debian
```bash
sudo apt update
sudo apt install -y qemu-system-x86 qemu-kvm
sudo usermod -aG kvm $USER
```

#### RHEL/Fedora
```bash
sudo dnf install -y qemu-kvm qemu-system-x86
sudo usermod -aG kvm $USER
```

## Installation

### Build from Source

```bash
git clone https://github.com/kariy/katana-hypervisor.git
cd katana-hypervisor
cargo build --release
```

### Build Boot Components

**Required before first use**: Build the shared VM boot components from the Katana repository.

```bash
# Build boot components (kernel + initrd + OVMF)
cd /path/to/katana
make build-tee

# Copy to hypervisor
cp output/{vmlinuz,initrd.img,ovmf.fd} /path/to/katana-hypervisor/boot-components/
```

See `boot-components/README.md` for details.

## Quick Start

### Create an Instance

```bash
# Create instance with default resources
katana-hypervisor create dev1

# Create with custom resources
katana-hypervisor create dev1 \
  --vcpus 4 \
  --memory 8G \
  --storage 20G \
  --port 5050

# Create and start immediately
katana-hypervisor create dev1 --auto-start
```

### Manage Instances

```bash
# List all instances
katana-hypervisor list

# Start instance
katana-hypervisor start dev1

# View logs
katana-hypervisor logs dev1
katana-hypervisor logs dev1 -f  # follow mode

# Stop instance
katana-hypervisor stop dev1

# Delete instance
katana-hypervisor delete dev1
```

### Test Instance

```bash
# Wait for katana to initialize (~5 seconds)
sleep 5

# Health check
curl http://localhost:5050/

# Get chain ID
curl -X POST http://localhost:5050 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"starknet_chainId","params":[],"id":1}'
```

## TEE Mode (AMD SEV-SNP)

### Calculate Expected Measurement

```bash
katana-hypervisor measure \
  --vcpus 4 \
  --vcpu-type EPYC-v4 \
  --katana-repo /path/to/katana
```

### Verify Attestation

```bash
# Create TEE instance (future feature)
katana-hypervisor create secure1 --tee --auto-start

# Verify attestation
katana-hypervisor attest secure1
```

## Configuration

### Instance Configuration

Each instance has isolated:
- **Data directory**: `~/.local/share/hypervisor/instances/<instance-id>/`
- **Serial log**: `serial.log` in instance directory
- **QMP socket**: `qmp.sock` in instance directory

### State Database

Location: `~/.local/share/hypervisor/state.db`

Contains:
- Instance configurations
- Port allocations
- Boot component hashes
- Expected measurements (TEE mode)

## Development

### Run Tests

```bash
# All tests
cargo test

# Specific test
cargo test test_full_instance_lifecycle

# With output
cargo test -- --nocapture
```

### Project Structure

```
katana-hypervisor/
├── src/
│   ├── cli/           # CLI commands
│   ├── instance/      # Instance management
│   ├── port/          # Port allocation
│   ├── qemu/          # QEMU/KVM integration
│   ├── state/         # State persistence
│   └── tee/           # TEE support (SEV-SNP)
├── boot-components/   # Shared VM boot files
└── tests/            # Integration tests
```

## Troubleshooting

### KVM Permission Denied

```bash
# Add user to kvm group
sudo usermod -aG kvm $USER
# Log out and back in
```

### Boot Components Missing

```bash
# Build boot components from katana repo
cd /path/to/katana && make build-tee
cp output/{vmlinuz,initrd.img,ovmf.fd} boot-components/
```

### Port Already in Use

```bash
# Specify different port
katana-hypervisor create dev1 --port 5051
```

### Instance Stuck in Starting State

```bash
# Check logs
katana-hypervisor logs <instance-name>

# Force delete and recreate
katana-hypervisor delete <instance-name> --force
```

## Comparison with Docker

| Feature | Katana Hypervisor | Docker |
|---------|------------------|--------|
| Isolation | Hardware (VM) | Namespace/cgroups |
| TEE Support | ✅ SEV-SNP | ❌ |
| Attestation | ✅ Remote attestation | ❌ |
| Boot Time | ~5s | ~1s |
| Memory Overhead | ~50MB | ~10MB |
| Use Case | Production, confidential computing | Development |

## Performance

- **Boot time**: ~5 seconds to Katana RPC ready
- **Memory overhead**: ~50MB per VM (QEMU + kernel)
- **Disk overhead**: ~40MB shared boot components + per-instance data

## Security

- **VM Isolation**: Each instance runs in a separate QEMU VM
- **Port Binding**: Defaults to 127.0.0.1 (localhost only)
- **File Permissions**: Instance directories with 700 permissions
- **SEV-SNP** (optional): Memory encryption and attestation

## Contributing

Contributions welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Add tests for new functionality
4. Ensure all tests pass: `cargo test`
5. Submit a pull request

## License

MIT License - see LICENSE file for details

## Related Projects

- [Katana](https://github.com/dojoengine/dojo) - Starknet sequencer
- [Dojo](https://github.com/dojoengine/dojo) - Provable game engine

## Acknowledgments

Built for the Dojo/Katana ecosystem to enable production deployments with hardware-level isolation and optional confidential computing support.

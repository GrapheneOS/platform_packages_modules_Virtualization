"""Generates a 64-byte instance ID for a VM interacting with Trusted HALs.

This script reads a JSON configuration file, constructs the instance ID based
on the defined structure, and writes the result to a binary file. It is
designed for use in build systems and is silent on success.
"""
import argparse
import json
import uuid
from typing import Any, Dict, TypedDict

class InstanceIdConfig(TypedDict):
    """Represents the structure of a VM Instance ID config after validation."""
    is_vm_persistent: bool
    vm_partition: str
    vm_primary_uuid: uuid.UUID
    vm_secondary_uuid: uuid.UUID

# Expected schema for the VM Instance ID configuration file.
INSTANCE_ID_SCHEMA: Dict[str, Dict[str, Any]] = {
    'is_vm_persistent': {
        'type': bool,
        'required': True,
    },
    'vm_partition': {
        'type': str,
        'required': True,
        'allowed_values': ['system', 'vendor'],
    },
    'vm_primary_uuid': {
        'type': uuid.UUID,
        'required': True,
    },
    'vm_secondary_uuid': {
        'type': uuid.UUID,
        'required': False,
        # Default to a "zero" UUID if not provided.
        'default': uuid.UUID('{00000000-0000-0000-0000-000000000000}'),
    },
}

def validate_config(raw_config: Dict[str, Any]) -> InstanceIdConfig:
    """Validates a raw config dict against the INSTANCE_ID_SCHEMA.

    This function checks for required fields, applies defaults for optional
    fields, and converts values to their expected types (e.g., string to UUID).

    Args:
        raw_config: The dictionary loaded from the JSON file.

    Returns:
        A validated dictionary that is guaranteed to conform to the schema.
    """
    unknown_fields = set(raw_config.keys()) - set(INSTANCE_ID_SCHEMA.keys())
    if unknown_fields:
        fields_str = ", ".join(f"'{field}'" for field in unknown_fields)
        raise ValueError(
            f"Configuration error: Unknown fields found: {fields_str}"
        )

    instance_id_config: Dict[str, Any] = {}
    for field, schema in INSTANCE_ID_SCHEMA.items():
        if schema['required'] and field not in raw_config:
            raise ValueError(
                f"Configuration error: Missing required field '{field}'."
            )

        value = raw_config.get(field, schema.get('default'))

        expected_type = schema['type']
        if expected_type == uuid.UUID and isinstance(value, str):
            try:
                value = uuid.UUID(value)
            except (ValueError, TypeError) as exc:
                raise ValueError(
                    f"Config error: '{value}' is not a valid UUID "
                    f"for '{field}'."
                ) from exc

        if not isinstance(value, expected_type):
            raise ValueError(
                f"Config error: Field '{field}' has wrong type. "
                f"Expected {expected_type.__name__}, "
                f"got {type(value).__name__}."
            )

        if 'allowed_values' in schema and value not in schema['allowed_values']:
            raise ValueError(
                f"Config error: Value '{value}' for field '{field}' is not "
                f"supported. Allowed values are: {schema['allowed_values']}"
            )

        instance_id_config[field] = value
    return InstanceIdConfig(**instance_id_config)

def generate_instance_id(config: InstanceIdConfig) -> bytearray:
    """Generates a 64-byte instance ID from a configuration dictionary.

    The 64-byte structure is defined as follows:
    - Bytes 0-15:   AVF Reserved Space
      - Byte 0 (MSB): Set to 1 if the VM is persistent.
      - Bytes 0-3:  Defines the owner partition ('system' supported).
      - Bytes 4-15: Reserved for future use, must be zero.
    - Bytes 16-31:  VM Identifier UUID (e.g., for Widevine, Test VM).
    - Bytes 32-63:  VM-Specific Space
      - Bytes 32-47: Optional UUID to differentiate a VM instance.
      - Bytes 48-63: Reserved for future use, must be zero.

    Args:
        config: A dictionary containing the VM configuration.

    Returns:
        A 64-byte bytearray representing the Instance ID.
    """
    # Initialize a 64-byte array with all zeros. This handles all reserved
    # and zero-padded fields by default.
    instance_id = bytearray(64)

    # --- Bytes 0-15: AVF Reserved Space ---
    # TODO(b/441000033): We will need to modify this code a little further
    # once we finish aligning on how to represent persistent/non persistent
    # VMs on system and vendor partitions.
    assert config['vm_partition'] in ['system', 'vendor'], \
        "Only 'system' or 'vendor' partitions are supported."

    if config['is_vm_persistent']:
        # Set the Most Significant Bit (MSB) of the first byte to 1.
        instance_id[0] |= 0x80

    if config['vm_partition'] == 'vendor':
        instance_id[0:4] = b'\xff\xff\xff\xff'

    # --- Bytes 16-63: UUIDs ---
    # The config is assumed to be valid at this point.
    instance_id[16:32] = config['vm_primary_uuid'].bytes
    instance_id[32:48] = config['vm_secondary_uuid'].bytes
    # Bytes 48-63 remain zeroed from initialization.

    return instance_id

def main() -> None:
    parser = argparse.ArgumentParser(
        description=(
            "Convert a JSON file to a 64-byte instance ID for a VM "
            "interacting with Trusted HALs. This script is designed for build "
            "systems and is silent on success."
        ),
    )
    parser.add_argument(
        "-i", "--input",
        dest="input_file",
        required=True,
        help="Path to the input JSON configuration file."
    )
    parser.add_argument(
        "-o", "--output",
        dest="output_file",
        required=True,
        help="Path to the output binary file."
    )
    args = parser.parse_args()

    with open(args.input_file, 'r', encoding="utf-8") as f:
        config_data = json.load(f)

    instance_id_config = validate_config(config_data)
    byte_array = generate_instance_id(instance_id_config)

    with open(args.output_file, 'wb') as f:
        f.write(byte_array)

if __name__ == "__main__":
    main()

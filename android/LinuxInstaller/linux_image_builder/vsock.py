#!/usr/bin/env python3

import socket

# Constants for vsock (from linux/vm_sockets.h)
AF_VSOCK = 40
SOCK_STREAM = 1
VMADDR_CID_ANY = -1

def get_local_ip():
    """Retrieves the first IPv4 address found on the system.

    Returns:
        str: The local IPv4 address, or '127.0.0.1' if no IPv4 address is found.
    """

    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(('8.8.8.8', 80))
        ip = s.getsockname()[0]
    except Exception:
        ip = '127.0.0.1'
    finally:
        s.close()
    return ip

def main():
    PORT = 1024

    # Create a vsock socket
    server_socket = socket.socket(AF_VSOCK, SOCK_STREAM)

    # Bind the socket to the server address
    server_address = (VMADDR_CID_ANY, PORT)
    server_socket.bind(server_address)

    # Listen for incoming connections
    server_socket.listen(1)
    print(f"VSOCK server listening on port {PORT}...")

    while True:
        # Accept a connection
        connection, client_address = server_socket.accept()
        print(f"Connection from: {client_address}")

        try:
            # Get the local IP address
            local_ip = get_local_ip()

            # Send the IP address to the client
            connection.sendall(local_ip.encode())
        finally:
            # Close the connection
            connection.close()

if __name__ == "__main__":
    main()

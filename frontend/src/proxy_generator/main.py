# Einstiegspunkt der Anwendung.
# Erzeugt die QApplication und zeigt das Hauptfenster an.

import logging
import sys

from PyQt6.QtWidgets import QApplication

from proxy_generator.app import create_app


def main() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    )

    app = QApplication(sys.argv)
    app.setApplicationName("Proxy Generator")
    app.setOrganizationName("proxy-generator")

    window = create_app()
    if window is None:
        sys.exit(1)

    window.show()
    sys.exit(app.exec())


if __name__ == "__main__":
    main()

# App-Factory: Baut das Hauptfenster und verbindet ViewModels mit Views.

import logging
import shutil
from typing import Optional

from PyQt6.QtWidgets import QMessageBox

from proxy_generator.ipc.client import IpcClient, _find_backend_binary
from proxy_generator.viewmodels.queue_viewmodel import QueueViewModel
from proxy_generator.views.main_window import MainWindow

log = logging.getLogger(__name__)


def create_app() -> Optional[MainWindow]:
    """Erzeugt und konfiguriert das Hauptfenster.

    Returns None if the backend binary cannot be found (shows error dialog).
    """
    backend_path = _find_backend_binary()

    # Check if backend binary is reachable
    if not _backend_available(backend_path):
        QMessageBox.critical(
            None,
            "Backend nicht gefunden",
            f"Das Backend konnte nicht gefunden werden.\n\n"
            f"Gesuchter Pfad: {backend_path}\n\n"
            f"Setze die Umgebungsvariable PROXY_GENERATOR_BACKEND auf den "
            f"korrekten Pfad oder stelle sicher, dass das Backend installiert ist.",
        )
        return None

    client = IpcClient(backend_path)
    try:
        client.start()
    except OSError as exc:
        QMessageBox.critical(
            None,
            "Backend-Startfehler",
            f"Das Backend konnte nicht gestartet werden:\n\n{exc}",
        )
        return None

    window = MainWindow()

    vm = QueueViewModel(client, parent=window)
    vm.start_worker()

    window.set_viewmodel(vm)
    return window


def _backend_available(path: str) -> bool:
    """Check whether the backend binary exists."""
    import os
    if os.path.isabs(path):
        return os.path.isfile(path)
    # On PATH
    return shutil.which(path) is not None

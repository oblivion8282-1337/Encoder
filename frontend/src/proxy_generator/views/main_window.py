# Hauptfenster der Anwendung.
# Enthaelt die Job-Tabelle, Einstellungen und Steuerungsbuttons.

from __future__ import annotations

import os
from pathlib import Path
from typing import Optional

from PyQt6.QtCore import Qt, QModelIndex
from PyQt6.QtGui import QAction
from PyQt6.QtWidgets import (
    QAbstractItemView,
    QButtonGroup,
    QComboBox,
    QFileDialog,
    QGroupBox,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QLineEdit,
    QMainWindow,
    QMenu,
    QMessageBox,
    QProgressBar,
    QPushButton,
    QRadioButton,
    QSizePolicy,
    QSpinBox,
    QSplitter,
    QStatusBar,
    QTableWidget,
    QTableWidgetItem,
    QToolBar,
    QVBoxLayout,
    QWidget,
)

from proxy_generator.models.job import Job, JobMode, JobOptions, JobStatus

VIDEO_EXTENSIONS = {".mp4", ".mov", ".mxf", ".mts", ".m2ts", ".avi", ".mkv", ".r3d", ".braw"}

_STATUS_LABELS = {
    JobStatus.QUEUED: "Wartend",
    JobStatus.RUNNING: "Laeuft",
    JobStatus.DONE: "Fertig",
    JobStatus.ERROR: "Fehler",
    JobStatus.CANCELLED: "Abgebrochen",
}

# Table column indices
COL_FILENAME = 0
COL_MODE = 1
COL_STATUS = 2
COL_PROGRESS = 3
COL_FPS = 4
COL_SPEED = 5
NUM_COLS = 6


class MainWindow(QMainWindow):
    """Hauptfenster des Proxy Generators."""

    def __init__(self, viewmodel=None) -> None:
        super().__init__()
        self._vm = viewmodel  # set later via set_viewmodel if None
        self.setWindowTitle("Proxy Generator")
        self.setMinimumSize(1000, 650)

        self._build_toolbar()
        self._build_central()
        self._build_statusbar()

        if self._vm is not None:
            self._connect_viewmodel()

    # -- public -----------------------------------------------------------------

    def set_viewmodel(self, vm) -> None:
        self._vm = vm
        self._connect_viewmodel()

    # -- toolbar ----------------------------------------------------------------

    def _build_toolbar(self) -> None:
        tb = QToolBar("Hauptleiste")
        tb.setMovable(False)
        self.addToolBar(tb)

        self._act_add_files = QAction("Dateien hinzufuegen", self)
        self._act_add_files.triggered.connect(self._on_add_files)
        tb.addAction(self._act_add_files)

        self._act_add_folder = QAction("Ordner hinzufuegen", self)
        self._act_add_folder.triggered.connect(self._on_add_folder)
        tb.addAction(self._act_add_folder)

        tb.addSeparator()

        self._act_start = QAction("Alles starten", self)
        self._act_start.triggered.connect(self._on_start_all)
        tb.addAction(self._act_start)

        self._act_clear = QAction("Fertige entfernen", self)
        self._act_clear.triggered.connect(self._on_clear_done)
        tb.addAction(self._act_clear)

    # -- central widget ---------------------------------------------------------

    def _build_central(self) -> None:
        splitter = QSplitter(Qt.Orientation.Horizontal)
        self.setCentralWidget(splitter)

        # Left: job table
        self._table = QTableWidget(0, NUM_COLS)
        self._table.setHorizontalHeaderLabels([
            "Dateiname", "Modus", "Status", "Fortschritt", "FPS", "Speed",
        ])
        self._table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self._table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self._table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self._table.customContextMenuRequested.connect(self._on_table_context_menu)
        header = self._table.horizontalHeader()
        header.setStretchLastSection(True)
        header.setSectionResizeMode(COL_FILENAME, QHeaderView.ResizeMode.Stretch)
        for col in (COL_MODE, COL_STATUS, COL_FPS, COL_SPEED):
            header.setSectionResizeMode(col, QHeaderView.ResizeMode.ResizeToContents)
        header.setSectionResizeMode(COL_PROGRESS, QHeaderView.ResizeMode.Fixed)
        self._table.setColumnWidth(COL_PROGRESS, 160)

        splitter.addWidget(self._table)

        # Right: settings panel
        settings_widget = self._build_settings_panel()
        splitter.addWidget(settings_widget)
        splitter.setStretchFactor(0, 3)
        splitter.setStretchFactor(1, 1)

    def _build_settings_panel(self) -> QWidget:
        container = QWidget()
        layout = QVBoxLayout(container)

        # -- Output directory ---------------------------------------------------
        grp_output = QGroupBox("Ausgabe")
        gl = QHBoxLayout(grp_output)
        self._output_dir_edit = QLineEdit()
        self._output_dir_edit.setPlaceholderText("Ausgabe-Ordner waehlen...")
        self._output_dir_edit.setText(str(Path.home()))
        gl.addWidget(self._output_dir_edit)
        btn_browse = QPushButton("Waehlen")
        btn_browse.clicked.connect(self._on_browse_output)
        gl.addWidget(btn_browse)
        layout.addWidget(grp_output)

        # -- Mode ---------------------------------------------------------------
        grp_mode = QGroupBox("Modus")
        ml = QVBoxLayout(grp_mode)
        self._mode_group = QButtonGroup(self)
        self._rb_rewrap = QRadioButton("Re-Wrap (nur Container)")
        self._rb_proxy = QRadioButton("Proxy (Transkodierung)")
        self._mode_group.addButton(self._rb_rewrap, 0)
        self._mode_group.addButton(self._rb_proxy, 1)
        self._rb_rewrap.setChecked(True)
        ml.addWidget(self._rb_rewrap)
        ml.addWidget(self._rb_proxy)
        layout.addWidget(grp_mode)

        # -- Proxy settings (only visible in proxy mode) ------------------------
        self._grp_proxy = QGroupBox("Proxy-Einstellungen")
        pl = QVBoxLayout(self._grp_proxy)

        pl.addWidget(QLabel("Aufloesung:"))
        self._combo_resolution = QComboBox()
        self._combo_resolution.addItems([
            "Beibehalten", "1920x1080", "1280x720", "960x540",
        ])
        pl.addWidget(self._combo_resolution)

        pl.addWidget(QLabel("Codec:"))
        self._combo_codec = QComboBox()
        self._combo_codec.addItems(["H.264", "DNxHR"])
        pl.addWidget(self._combo_codec)

        layout.addWidget(self._grp_proxy)
        self._grp_proxy.setVisible(False)
        self._mode_group.idToggled.connect(self._on_mode_changed)

        # -- Hardware encoding --------------------------------------------------
        grp_hw = QGroupBox("Hardware-Encoding")
        hl = QVBoxLayout(grp_hw)
        self._combo_hw = QComboBox()
        self._combo_hw.addItems(["Keins", "NVENC (Nvidia)", "VAAPI (AMD/Intel)"])
        hl.addWidget(self._combo_hw)
        layout.addWidget(grp_hw)

        # -- Parallel jobs ------------------------------------------------------
        grp_par = QGroupBox("Parallele Jobs")
        prl = QVBoxLayout(grp_par)
        self._spin_parallel = QSpinBox()
        self._spin_parallel.setRange(1, 8)
        self._spin_parallel.setValue(1)
        prl.addWidget(self._spin_parallel)
        layout.addWidget(grp_par)

        layout.addStretch()
        return container

    # -- statusbar --------------------------------------------------------------

    def _build_statusbar(self) -> None:
        self._statusbar = QStatusBar()
        self.setStatusBar(self._statusbar)
        self._status_label = QLabel("0 Jobs")
        self._statusbar.addPermanentWidget(self._status_label)

    # -- connect viewmodel signals ----------------------------------------------

    def _connect_viewmodel(self) -> None:
        if self._vm is None:
            return
        self._vm.jobs_changed.connect(self._rebuild_table)
        self._vm.job_updated.connect(self._update_job_row)

    # -- toolbar actions --------------------------------------------------------

    def _on_add_files(self) -> None:
        exts = " ".join(f"*{e}" for e in sorted(VIDEO_EXTENSIONS))
        paths, _ = QFileDialog.getOpenFileNames(
            self, "Videodateien auswaehlen", "",
            f"Videodateien ({exts});;Alle Dateien (*)",
        )
        if paths:
            self._submit_jobs(paths)

    def _on_add_folder(self) -> None:
        folder = QFileDialog.getExistingDirectory(self, "Ordner auswaehlen")
        if not folder:
            return
        paths = []
        for root, _dirs, files in os.walk(folder):
            for f in files:
                if Path(f).suffix.lower() in VIDEO_EXTENSIONS:
                    paths.append(os.path.join(root, f))
        if paths:
            self._submit_jobs(sorted(paths))
        else:
            QMessageBox.information(self, "Keine Videos", "Keine Videodateien im Ordner gefunden.")

    def _on_start_all(self) -> None:
        if self._vm is None:
            return
        # Worker is already started by the app on startup.
        # Jobs are sent to the backend immediately when added.
        self._statusbar.showMessage("Jobs werden automatisch verarbeitet.", 3000)

    def _on_clear_done(self) -> None:
        if self._vm is None:
            return
        self._vm.clear_done()

    def _on_browse_output(self) -> None:
        folder = QFileDialog.getExistingDirectory(self, "Ausgabe-Ordner waehlen")
        if folder:
            self._output_dir_edit.setText(folder)

    def _on_mode_changed(self, button_id: int, checked: bool) -> None:
        if checked:
            self._grp_proxy.setVisible(button_id == 1)

    # -- context menu -----------------------------------------------------------

    def _on_table_context_menu(self, pos) -> None:
        row = self._table.rowAt(pos.y())
        if row < 0:
            return
        menu = QMenu(self)
        act_cancel = menu.addAction("Abbrechen")
        act_remove = menu.addAction("Entfernen")
        action = menu.exec(self._table.viewport().mapToGlobal(pos))
        if action is None or self._vm is None:
            return
        jobs = self._vm.jobs
        if row >= len(jobs):
            return
        job = jobs[row]
        if action == act_cancel:
            self._vm.cancel_job(job.id)
        elif action == act_remove:
            self._vm.remove_job(job.id)

    # -- submit jobs (gather options from settings panel) -----------------------

    def _submit_jobs(self, paths: list[str]) -> None:
        if self._vm is None:
            return

        output_dir = self._output_dir_edit.text().strip()
        if not output_dir:
            QMessageBox.warning(self, "Ausgabe-Ordner", "Bitte einen Ausgabe-Ordner waehlen.")
            return
        if not os.path.isdir(output_dir):
            QMessageBox.warning(self, "Ausgabe-Ordner", f"Ordner existiert nicht:\n{output_dir}")
            return
        if not os.access(output_dir, os.W_OK):
            QMessageBox.warning(self, "Ausgabe-Ordner", f"Kein Schreibzugriff auf:\n{output_dir}")
            return

        mode = JobMode.PROXY if self._rb_proxy.isChecked() else JobMode.REWRAP

        resolution_text = self._combo_resolution.currentText()
        proxy_resolution: Optional[str] = None
        if resolution_text != "Beibehalten":
            proxy_resolution = resolution_text

        codec_map = {"H.264": "h264", "DNxHR": "dnxhr"}
        proxy_codec = codec_map.get(self._combo_codec.currentText(), "h264")

        hw_map = {"Keins": "none", "NVENC (Nvidia)": "nvenc", "VAAPI (AMD/Intel)": "vaapi"}
        hw_accel = hw_map.get(self._combo_hw.currentText(), "none")

        options = JobOptions(
            proxy_resolution=proxy_resolution,
            proxy_codec=proxy_codec,
            hw_accel=hw_accel,
        )

        self._vm.set_parallel_jobs(self._spin_parallel.value())
        self._vm.add_files(paths, output_dir, mode, options)

    # -- table management -------------------------------------------------------

    def _rebuild_table(self) -> None:
        """Rebuild the entire table from the viewmodel's job list."""
        if self._vm is None:
            return
        jobs = self._vm.jobs
        self._table.setRowCount(len(jobs))
        for row, job in enumerate(jobs):
            self._set_row(row, job)
        self._update_status_label()

    def _update_job_row(self, job_id: str) -> None:
        """Update a single row in-place by job id."""
        if self._vm is None:
            return
        jobs = self._vm.jobs
        for row, job in enumerate(jobs):
            if job.id == job_id:
                self._set_row(row, job)
                break
        self._update_status_label()

    def _set_row(self, row: int, job: Job) -> None:
        filename = Path(job.input_path).name
        mode_label = "Re-Wrap" if job.mode == JobMode.REWRAP else "Proxy"
        status_label = _STATUS_LABELS.get(job.status, str(job.status))

        self._table.setItem(row, COL_FILENAME, QTableWidgetItem(filename))
        self._table.setItem(row, COL_MODE, QTableWidgetItem(mode_label))

        status_item = QTableWidgetItem(status_label)
        if job.status == JobStatus.ERROR:
            status_item.setToolTip(job.error or "")
        self._table.setItem(row, COL_STATUS, status_item)

        # Progress bar
        bar = self._table.cellWidget(row, COL_PROGRESS)
        if not isinstance(bar, QProgressBar):
            bar = QProgressBar()
            bar.setRange(0, 100)
            bar.setTextVisible(True)
            self._table.setCellWidget(row, COL_PROGRESS, bar)
        bar.setValue(int(job.progress))

        fps_text = f"{job.fps:.1f}" if job.fps > 0 else ""
        self._table.setItem(row, COL_FPS, QTableWidgetItem(fps_text))

        speed_text = f"{job.speed:.2f}x" if job.speed > 0 else ""
        self._table.setItem(row, COL_SPEED, QTableWidgetItem(speed_text))

    def _update_status_label(self) -> None:
        if self._vm is None:
            return
        jobs = self._vm.jobs
        total = len(jobs)
        running = sum(1 for j in jobs if j.status == JobStatus.RUNNING)
        done = sum(1 for j in jobs if j.status == JobStatus.DONE)
        self._status_label.setText(f"{total} Jobs | {running} laufen | {done} fertig")

    # -- cleanup ----------------------------------------------------------------

    def closeEvent(self, event) -> None:
        if self._vm is not None:
            self._vm.stop()
        super().closeEvent(event)

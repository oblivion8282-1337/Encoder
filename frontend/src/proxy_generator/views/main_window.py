# Hauptfenster der Anwendung.
# Enthaelt die Job-Tabelle, Einstellungen und Steuerungsbuttons.

from __future__ import annotations

import os
from pathlib import Path
from typing import Optional

from PyQt6.QtCore import Qt, QModelIndex, QSettings
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
        self._settings = QSettings("proxy-generator", "ProxyGenerator")
        self.setWindowTitle("Proxy Generator")
        self.setMinimumSize(1000, 650)
        self.setAcceptDrops(True)

        self._build_toolbar()
        self._build_central()
        self._build_statusbar()
        self._load_settings()

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

        self._act_clear_all = QAction("Queue leeren", self)
        self._act_clear_all.triggered.connect(
            lambda: self._vm.clear_all() if self._vm else None)
        tb.addAction(self._act_clear_all)

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
        self._table.cellDoubleClicked.connect(self._on_cell_double_clicked)
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
            "Beibehalten", "1920:1080", "1280:720", "960:540",
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
        if not self._vm.jobs:
            QMessageBox.information(self, "Queue leer",
                "Bitte zuerst Dateien ueber 'Dateien hinzufuegen' hinzufuegen.")
            return
        # Worker is already started by the app on startup.
        # Jobs are sent to the backend immediately when added.
        self._statusbar.showMessage("Jobs werden verarbeitet...", 3000)

    def _on_clear_done(self) -> None:
        if self._vm is None:
            return
        self._vm.clear_done()

    def _on_browse_output(self) -> None:
        folder = QFileDialog.getExistingDirectory(self, "Ausgabe-Ordner waehlen")
        if folder:
            self._output_dir_edit.setText(folder)

    def _on_mode_changed(self, button_id: int = -1, checked: bool = True) -> None:
        if checked:
            if button_id == -1:
                # Called directly (e.g. from _load_settings) - check current state
                self._grp_proxy.setVisible(self._rb_proxy.isChecked())
            else:
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

        # TODO: Backend unterstuetzt set_parallel_jobs noch nicht (kein Request-Typ in protocol.rs).
        # self._vm.set_parallel_jobs(self._spin_parallel.value())
        count_before = len(self._vm.jobs)
        self._vm.add_files(paths, output_dir, mode, options)
        count_after = len(self._vm.jobs)
        added = count_after - count_before
        if added == 0 and paths:
            QMessageBox.warning(self, "Fehler",
                "Keine Jobs konnten hinzugefuegt werden.\nPruefen Sie ob das Backend laeuft.")
        elif added < len(paths):
            self._statusbar.showMessage(
                f"{added} von {len(paths)} Jobs hinzugefuegt.", 5000)

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
            status_item.setForeground(Qt.GlobalColor.red)
        elif job.status == JobStatus.DONE:
            status_item.setForeground(Qt.GlobalColor.darkGreen)
        elif job.status == JobStatus.CANCELLED:
            status_item.setForeground(Qt.GlobalColor.gray)
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

    # -- settings persistence ---------------------------------------------------

    def _load_settings(self) -> None:
        self._output_dir_edit.setText(
            self._settings.value("output_dir", str(Path.home())))
        mode = self._settings.value("mode", "rewrap")
        if mode == "proxy":
            self._rb_proxy.setChecked(True)
        else:
            self._rb_rewrap.setChecked(True)
        self._on_mode_changed()
        res = self._settings.value("proxy_resolution", "Beibehalten")
        idx = self._combo_resolution.findText(res)
        if idx >= 0:
            self._combo_resolution.setCurrentIndex(idx)
        codec = self._settings.value("proxy_codec", "H.264")
        idx = self._combo_codec.findText(codec)
        if idx >= 0:
            self._combo_codec.setCurrentIndex(idx)
        hw = self._settings.value("hw_accel", "Keins")
        idx = self._combo_hw.findText(hw)
        if idx >= 0:
            self._combo_hw.setCurrentIndex(idx)
        parallel = int(self._settings.value("parallel_jobs", 1))
        self._spin_parallel.setValue(parallel)

    def _save_settings(self) -> None:
        self._settings.setValue("output_dir", self._output_dir_edit.text())
        mode = "proxy" if self._rb_proxy.isChecked() else "rewrap"
        self._settings.setValue("mode", mode)
        self._settings.setValue("proxy_resolution",
                                self._combo_resolution.currentText())
        self._settings.setValue("proxy_codec",
                                self._combo_codec.currentText())
        self._settings.setValue("hw_accel", self._combo_hw.currentText())
        self._settings.setValue("parallel_jobs", self._spin_parallel.value())

    # -- drag & drop ------------------------------------------------------------

    def dragEnterEvent(self, event) -> None:
        if event.mimeData().hasUrls():
            event.acceptProposedAction()

    def dropEvent(self, event) -> None:
        paths = []
        for url in event.mimeData().urls():
            path = url.toLocalFile()
            if Path(path).suffix.lower() in VIDEO_EXTENSIONS:
                paths.append(path)
            elif Path(path).is_dir():
                for f in Path(path).rglob("*"):
                    if f.suffix.lower() in VIDEO_EXTENSIONS:
                        paths.append(str(f))
        if paths:
            self._submit_jobs(paths)

    # -- error dialog on double-click -------------------------------------------

    def _on_cell_double_clicked(self, row: int, col: int) -> None:
        if self._vm is None:
            return
        jobs = self._vm.jobs
        if row >= len(jobs):
            return
        job = jobs[row]
        if job.status == JobStatus.ERROR and job.error:
            QMessageBox.warning(
                self,
                "Job-Fehler",
                f"Datei: {Path(job.input_path).name}\n\nFehler:\n{job.error}",
            )

    # -- cleanup ----------------------------------------------------------------

    def closeEvent(self, event) -> None:
        self._save_settings()
        if self._vm is not None:
            self._vm.stop()
        super().closeEvent(event)

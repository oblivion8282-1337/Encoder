# Hauptfenster der Anwendung.
# Enthaelt die Job-Tabelle, Einstellungen und Steuerungsbuttons.

from __future__ import annotations

import os
from pathlib import Path
from typing import Optional

from PyQt6.QtCore import Qt, QEvent, QSettings
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
    QCheckBox,
    QProgressBar,
    QPushButton,
    QRadioButton,
    QSpinBox,
    QSplitter,
    QStatusBar,
    QTableWidget,
    QTableWidgetItem,
    QToolBar,
    QVBoxLayout,
    QWidget,
)

from proxy_generator.i18n import get_language, set_language, tr
from proxy_generator.models.job import Job, JobMode, JobOptions, JobStatus

VIDEO_EXTENSIONS = {".mp4", ".mov", ".mxf", ".mts", ".m2ts", ".avi", ".mkv", ".r3d", ".braw"}

# Table column indices
COL_FILENAME = 0
COL_MODE = 1
COL_STATUS = 2
COL_PROGRESS = 3
COL_FPS = 4
COL_SPEED = 5
NUM_COLS = 6

_STATUS_KEY = {
    JobStatus.QUEUED:    "status.waiting",
    JobStatus.RUNNING:   "status.running",
    JobStatus.DONE:      "status.done",
    JobStatus.ERROR:     "status.error",
    JobStatus.CANCELLED: "status.cancelled",
}


class MainWindow(QMainWindow):
    """Hauptfenster des Proxy Generators."""

    def __init__(self, viewmodel=None) -> None:
        super().__init__()
        self._vm = viewmodel  # set later via set_viewmodel if None
        self._settings = QSettings("proxy-generator", "ProxyGenerator")
        self.setMinimumSize(1000, 650)
        self.setAcceptDrops(True)

        self._build_toolbar()
        self._build_central()
        self._build_statusbar()
        self._load_settings()
        self.retranslate_ui()

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

        self._act_add_files = QAction("", self)
        self._act_add_files.triggered.connect(self._on_add_files)
        tb.addAction(self._act_add_files)

        self._act_add_folder = QAction("", self)
        self._act_add_folder.triggered.connect(self._on_add_folder)
        tb.addAction(self._act_add_folder)

        tb.addSeparator()

        self._act_start = QAction("", self)
        self._act_start.triggered.connect(self._on_start_pause_resume)
        tb.addAction(self._act_start)
        self._start_state = "idle"  # "idle" | "running" | "paused"

        self._act_clear = QAction("", self)
        self._act_clear.triggered.connect(self._on_clear_done)
        tb.addAction(self._act_clear)

        self._act_clear_all = QAction("", self)
        self._act_clear_all.triggered.connect(
            lambda: self._vm.clear_all() if self._vm else None)
        tb.addAction(self._act_clear_all)

    # -- central widget ---------------------------------------------------------

    def _build_central(self) -> None:
        splitter = QSplitter(Qt.Orientation.Horizontal)
        self.setCentralWidget(splitter)

        # Left: job table
        self._table = QTableWidget(0, NUM_COLS)
        self._table.setHorizontalHeaderLabels([""] * NUM_COLS)
        self._table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self._table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self._table.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self._table.customContextMenuRequested.connect(self._on_table_context_menu)
        self._table.cellDoubleClicked.connect(self._on_cell_double_clicked)
        self._table.installEventFilter(self)
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
        self._grp_output = QGroupBox("")
        gl = QHBoxLayout(self._grp_output)
        self._output_dir_edit = QLineEdit()
        self._output_dir_edit.setText(str(Path.home()))
        gl.addWidget(self._output_dir_edit)
        self._btn_browse = QPushButton("")
        self._btn_browse.clicked.connect(self._on_browse_output)
        gl.addWidget(self._btn_browse)
        layout.addWidget(self._grp_output)

        # -- Mode ---------------------------------------------------------------
        self._grp_mode = QGroupBox("")
        ml = QVBoxLayout(self._grp_mode)
        self._mode_group = QButtonGroup(self)
        self._rb_rewrap = QRadioButton("")
        self._rb_proxy = QRadioButton("")
        self._mode_group.addButton(self._rb_rewrap, 0)
        self._mode_group.addButton(self._rb_proxy, 1)
        self._rb_rewrap.setChecked(True)
        ml.addWidget(self._rb_rewrap)
        ml.addWidget(self._rb_proxy)
        layout.addWidget(self._grp_mode)

        # -- Proxy settings (only visible in proxy mode) ------------------------
        self._grp_proxy = QGroupBox("")
        pl = QVBoxLayout(self._grp_proxy)

        self._lbl_resolution = QLabel("")
        pl.addWidget(self._lbl_resolution)
        self._combo_resolution = QComboBox()
        self._combo_resolution.addItems(["Original", "1/2", "1/4", "1/8"])
        pl.addWidget(self._combo_resolution)

        self._lbl_codec = QLabel("")
        pl.addWidget(self._lbl_codec)
        self._combo_codec = QComboBox()
        self._combo_codec.addItems([
            "H.264", "H.265", "AV1",
            "ProRes 422 Proxy", "ProRes 422 LT", "ProRes 422", "ProRes 422 HQ",
        ])
        self._combo_codec.currentTextChanged.connect(self._on_codec_changed)
        pl.addWidget(self._combo_codec)

        layout.addWidget(self._grp_proxy)
        self._grp_proxy.setVisible(False)
        self._mode_group.idToggled.connect(self._on_mode_changed)

        # -- Output naming ------------------------------------------------------
        self._grp_naming = QGroupBox("")
        nl = QVBoxLayout(self._grp_naming)
        self._lbl_suffix = QLabel("")
        nl.addWidget(self._lbl_suffix)
        self._edit_suffix = QLineEdit()
        nl.addWidget(self._edit_suffix)
        self._lbl_subfolder = QLabel("")
        nl.addWidget(self._lbl_subfolder)
        subfolder_row = QHBoxLayout()
        self._edit_subfolder = QLineEdit()
        subfolder_row.addWidget(self._edit_subfolder)
        self._btn_subfolder = QPushButton("")
        self._btn_subfolder.clicked.connect(self._on_browse_subfolder)
        subfolder_row.addWidget(self._btn_subfolder)
        nl.addLayout(subfolder_row)
        self._chk_skip_existing = QCheckBox("")
        nl.addWidget(self._chk_skip_existing)
        layout.addWidget(self._grp_naming)

        # -- Hardware encoding --------------------------------------------------
        self._grp_hw = QGroupBox("")
        hl = QVBoxLayout(self._grp_hw)
        self._combo_hw = QComboBox()
        self._combo_hw.addItems(["Keins / None", "NVENC (Nvidia)", "VAAPI (AMD/Intel)"])
        hl.addWidget(self._combo_hw)
        layout.addWidget(self._grp_hw)

        # -- Parallel jobs ------------------------------------------------------
        self._grp_par = QGroupBox("")
        prl = QVBoxLayout(self._grp_par)
        cpu_count = os.cpu_count() or 8
        par_row = QHBoxLayout()
        self._spin_parallel = QSpinBox()
        self._spin_parallel.setRange(1, cpu_count)
        self._spin_parallel.setValue(1)
        self._spin_parallel.valueChanged.connect(self._on_parallel_changed)
        par_row.addWidget(self._spin_parallel)
        par_row.addWidget(QLabel(f"/ {cpu_count}"))
        par_row.addStretch()
        prl.addLayout(par_row)
        layout.addWidget(self._grp_par)

        # -- Language -----------------------------------------------------------
        self._grp_lang = QGroupBox("")
        ll = QVBoxLayout(self._grp_lang)
        self._combo_lang = QComboBox()
        self._combo_lang.addItems(["Deutsch", "English"])
        self._combo_lang.currentIndexChanged.connect(self._on_language_changed)
        ll.addWidget(self._combo_lang)
        layout.addWidget(self._grp_lang)

        layout.addStretch()
        return container

    # -- statusbar --------------------------------------------------------------

    def _build_statusbar(self) -> None:
        self._statusbar = QStatusBar()
        self.setStatusBar(self._statusbar)
        self._status_label = QLabel("")
        self._statusbar.addPermanentWidget(self._status_label)

    # -- retranslate ------------------------------------------------------------

    def retranslate_ui(self) -> None:
        """Alle sichtbaren Texte in der aktuellen Sprache setzen."""
        self.setWindowTitle("Proxy Generator")

        # Toolbar
        self._act_add_files.setText(tr("toolbar.add_files"))
        self._act_add_folder.setText(tr("toolbar.add_folder"))
        self._set_start_state(self._start_state)
        self._act_clear.setText(tr("toolbar.clear_done"))
        self._act_clear_all.setText(tr("toolbar.clear_all"))

        # Table headers
        self._table.setHorizontalHeaderLabels([
            tr("table.filename"),
            tr("table.mode"),
            tr("table.status"),
            tr("table.progress"),
            tr("table.fps"),
            tr("table.speed"),
        ])

        # Settings panel
        self._grp_output.setTitle(tr("grp.output"))
        self._output_dir_edit.setPlaceholderText(tr("placeholder.output"))
        self._btn_browse.setText(tr("btn.browse"))
        self._grp_mode.setTitle(tr("grp.mode"))
        self._rb_rewrap.setText(tr("rb.rewrap"))
        self._rb_proxy.setText(tr("rb.proxy"))
        self._grp_proxy.setTitle(tr("grp.proxy"))
        self._lbl_resolution.setText(tr("lbl.resolution"))
        self._lbl_codec.setText(tr("lbl.codec"))
        self._grp_naming.setTitle(tr("grp.naming"))
        self._lbl_suffix.setText(tr("lbl.suffix"))
        self._edit_suffix.setPlaceholderText(tr("placeholder.suffix"))
        self._lbl_subfolder.setText(tr("lbl.subfolder"))
        self._edit_subfolder.setPlaceholderText(tr("placeholder.subfolder"))
        self._btn_subfolder.setText(tr("btn.browse"))
        self._chk_skip_existing.setText(tr("chk.skip_existing"))
        self._grp_hw.setTitle(tr("grp.hw"))
        self._grp_par.setTitle(tr("grp.parallel"))
        self._grp_lang.setTitle(tr("grp.lang"))

        # ProRes tooltip (might currently be set)
        self._on_codec_changed(self._combo_codec.currentText())

        # Refresh table content (status/mode columns)
        if self._vm is not None:
            self._rebuild_table()
        self._update_status_label()

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
            self,
            tr("fdlg.select_files"),
            "",
            f"{tr('fdlg.video_filter')} ({exts});;{tr('fdlg.all_files')} (*)",
        )
        if paths:
            self._submit_jobs(paths)

    def _on_add_folder(self) -> None:
        folder = QFileDialog.getExistingDirectory(self, tr("fdlg.select_folder"))
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
            QMessageBox.information(self, tr("dlg.no_videos_title"), tr("dlg.no_videos_msg"))

    def _gather_options(self) -> JobOptions:
        """Liest alle aktuellen Einstellungen aus der UI und gibt JobOptions zurück."""
        resolution_map = {
            "Original": None,
            "1/2": "iw/2:-2",
            "1/4": "iw/4:-2",
            "1/8": "iw/8:-2",
        }
        codec_map = {
            "H.264": "h264", "H.265": "h265", "AV1": "av1",
            "ProRes 422 Proxy": "prores_proxy", "ProRes 422 LT": "prores_lt",
            "ProRes 422": "prores_422", "ProRes 422 HQ": "prores_hq",
        }
        hw_map = {
            "Keins / None": "none",
            "NVENC (Nvidia)": "nvenc",
            "VAAPI (AMD/Intel)": "vaapi",
        }
        return JobOptions(
            proxy_resolution=resolution_map.get(self._combo_resolution.currentText()),
            proxy_codec=codec_map.get(self._combo_codec.currentText(), "h264"),
            hw_accel=hw_map.get(self._combo_hw.currentText(), "none"),
            output_suffix=self._edit_suffix.text(),
            output_subfolder=self._edit_subfolder.text().strip(),
            skip_if_exists=self._chk_skip_existing.isChecked(),
        )

    def _on_start_pause_resume(self) -> None:
        if self._vm is None:
            return
        if self._start_state == "idle":
            if not self._vm.jobs:
                QMessageBox.information(self, tr("dlg.queue_empty_title"), tr("dlg.queue_empty_msg"))
                return
            mode = JobMode.PROXY if self._rb_proxy.isChecked() else JobMode.REWRAP
            self._vm.start_all(mode, self._gather_options())
            self._set_start_state("running")
            self._statusbar.showMessage(tr("statusbar.started"), 3000)
        elif self._start_state == "running":
            self._vm.pause_all()
            self._set_start_state("paused")
        elif self._start_state == "paused":
            self._vm.resume_all()
            self._set_start_state("running")

    def _set_start_state(self, state: str) -> None:
        self._start_state = state
        if state == "idle":
            self._act_start.setText(tr("toolbar.start_all"))
        elif state == "running":
            self._act_start.setText(tr("toolbar.pause"))
        elif state == "paused":
            self._act_start.setText(tr("toolbar.resume"))

    def _on_clear_done(self) -> None:
        if self._vm is None:
            return
        self._vm.clear_done()

    def _on_browse_output(self) -> None:
        folder = QFileDialog.getExistingDirectory(self, tr("fdlg.output_folder"))
        if folder:
            self._output_dir_edit.setText(folder)

    def _on_browse_subfolder(self) -> None:
        # Dialog startet im aktuellen Ausgabe-Ordner (falls vorhanden)
        start = self._output_dir_edit.text().strip() or str(Path.home())
        folder = QFileDialog.getExistingDirectory(self, tr("fdlg.output_folder"), start)
        if folder:
            self._edit_subfolder.setText(folder)

    def _on_codec_changed(self, text: str) -> None:
        is_prores = text.startswith("ProRes")
        self._combo_hw.setEnabled(not is_prores)
        self._combo_hw.setToolTip(tr("tooltip.prores_cpu") if is_prores else "")

    def _on_mode_changed(self, button_id: int = -1, checked: bool = True) -> None:
        if checked:
            if button_id == -1:
                self._grp_proxy.setVisible(self._rb_proxy.isChecked())
            else:
                self._grp_proxy.setVisible(button_id == 1)

    def _on_parallel_changed(self, value: int) -> None:
        if self._vm is not None:
            self._vm.set_max_parallel(value)

    def _on_language_changed(self, index: int) -> None:
        lang = "de" if index == 0 else "en"
        set_language(lang)
        self._settings.setValue("language", lang)
        self.retranslate_ui()

    # -- context menu -----------------------------------------------------------

    def _on_table_context_menu(self, pos) -> None:
        row = self._table.rowAt(pos.y())
        if row < 0:
            return
        jobs = self._vm.jobs if self._vm else []
        if row >= len(jobs):
            return
        job = jobs[row]

        menu = QMenu(self)
        act_reset = menu.addAction(tr("ctx.reset"))
        act_reset.setEnabled(
            job.status in (JobStatus.DONE, JobStatus.ERROR, JobStatus.CANCELLED))
        act_cancel = menu.addAction(tr("ctx.cancel"))
        act_cancel.setEnabled(
            job.status in (JobStatus.QUEUED, JobStatus.RUNNING))
        act_remove = menu.addAction(tr("ctx.remove"))

        action = menu.exec(self._table.viewport().mapToGlobal(pos))
        if action is None or self._vm is None:
            return
        if action == act_reset:
            self._vm.reset_job(job.id)
        elif action == act_cancel:
            self._vm.cancel_job(job.id)
        elif action == act_remove:
            self._vm.remove_job(job.id)

    # -- submit jobs (gather options from settings panel) -----------------------

    def _submit_jobs(self, paths: list[str]) -> None:
        if self._vm is None:
            return

        output_dir = self._output_dir_edit.text().strip()
        if not output_dir:
            QMessageBox.warning(self, tr("dlg.output_title"), tr("dlg.output_choose"))
            return
        if not os.path.isdir(output_dir):
            QMessageBox.warning(self, tr("dlg.output_title"),
                                tr("dlg.output_not_found") + output_dir)
            return
        if not os.access(output_dir, os.W_OK):
            QMessageBox.warning(self, tr("dlg.output_title"),
                                tr("dlg.output_no_write") + output_dir)
            return

        mode = JobMode.PROXY if self._rb_proxy.isChecked() else JobMode.REWRAP
        options = self._gather_options()

        count_before = len(self._vm.jobs)
        self._vm.add_files(paths, output_dir, mode, options)
        count_after = len(self._vm.jobs)
        added = count_after - count_before
        if added == 0 and paths:
            QMessageBox.warning(self, tr("dlg.error_title"), tr("dlg.no_jobs_added"))
        elif added < len(paths):
            self._statusbar.showMessage(
                tr("statusbar.added").format(added=added, total=len(paths)), 5000)

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
        status_label = tr(_STATUS_KEY.get(job.status, "status.error"))

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
            self._status_label.setText("")
            return
        jobs = self._vm.jobs
        total = len(jobs)
        running = sum(1 for j in jobs if j.status == JobStatus.RUNNING)
        done = sum(1 for j in jobs if j.status == JobStatus.DONE)
        self._status_label.setText(
            tr("statusbar.summary").format(total=total, running=running, done=done))
        # Button zurücksetzen wenn keine Jobs mehr laufen oder warten
        if self._start_state in ("running", "paused"):
            active = any(j.status in (JobStatus.QUEUED, JobStatus.RUNNING) for j in jobs)
            if not active:
                self._set_start_state("idle")

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
        res = self._settings.value("proxy_resolution", "Original")
        idx = self._combo_resolution.findText(res)
        if idx >= 0:
            self._combo_resolution.setCurrentIndex(idx)
        codec = self._settings.value("proxy_codec", "H.264")
        idx = self._combo_codec.findText(codec)
        if idx >= 0:
            self._combo_codec.setCurrentIndex(idx)
        hw = self._settings.value("hw_accel", "Keins / None")
        idx = self._combo_hw.findText(hw)
        if idx >= 0:
            self._combo_hw.setCurrentIndex(idx)
        try:
            parallel = int(self._settings.value("parallel_jobs", 1))
        except (ValueError, TypeError):
            parallel = 1
        self._spin_parallel.setValue(parallel)
        self._edit_suffix.setText(self._settings.value("output_suffix", ""))
        self._edit_subfolder.setText(self._settings.value("output_subfolder", ""))
        self._chk_skip_existing.setChecked(
            self._settings.value("skip_if_exists", False, type=bool))
        # Language (must be loaded before retranslate_ui is called)
        lang = self._settings.value("language", "de")
        set_language(lang)
        lang_idx = 0 if lang == "de" else 1
        self._combo_lang.blockSignals(True)
        self._combo_lang.setCurrentIndex(lang_idx)
        self._combo_lang.blockSignals(False)

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
        self._settings.setValue("output_suffix", self._edit_suffix.text())
        self._settings.setValue("output_subfolder", self._edit_subfolder.text().strip())
        self._settings.setValue("skip_if_exists", self._chk_skip_existing.isChecked())
        self._settings.setValue("language", get_language())

    # -- drag & drop ------------------------------------------------------------

    def dragEnterEvent(self, event) -> None:
        if event.mimeData().hasUrls():
            event.acceptProposedAction()

    def dropEvent(self, event) -> None:
        paths = []
        for url in event.mimeData().urls():
            path = url.toLocalFile()
            if Path(path).is_file() and Path(path).suffix.lower() in VIDEO_EXTENSIONS:
                paths.append(path)
            elif Path(path).is_dir():
                for f in Path(path).rglob("*"):
                    if f.is_file() and f.suffix.lower() in VIDEO_EXTENSIONS:
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
                tr("dlg.job_error_title"),
                f"{Path(job.input_path).name}\n\n{job.error}",
            )

    # -- keyboard shortcuts -----------------------------------------------------

    def eventFilter(self, source, event) -> bool:
        if (source is self._table
                and event.type() == QEvent.Type.KeyPress
                and event.key() == Qt.Key.Key_Delete
                and self._vm is not None):
            for row in sorted(
                {idx.row() for idx in self._table.selectedIndexes()}, reverse=True
            ):
                jobs = self._vm.jobs
                if row < len(jobs):
                    self._vm.remove_job(jobs[row].id)
            return True
        return super().eventFilter(source, event)

    # -- cleanup ----------------------------------------------------------------

    def closeEvent(self, event) -> None:
        self._save_settings()
        if self._vm is not None:
            self._vm.stop()
        super().closeEvent(event)

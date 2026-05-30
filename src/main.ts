import { invoke } from "@tauri-apps/api/core";

interface PrintLog {
  timestamp: string;
  document_type: string;
  printer_name: string;
  print_engine: string;
  status: string;
  details: string;
}

interface DriverSettings {
  defaultPrinter: string | null;
  autoReconnect: boolean;
}

// State cache
let selectedPrinterName = "";
let defaultPrinterName = "";

window.addEventListener("DOMContentLoaded", () => {
  // Elements
  const printersContainer = document.getElementById("printers-container");
  const logsTbody = document.getElementById("logs-tbody");
  const logCountSpan = document.getElementById("log-count");
  
  const btnRefreshPrinters = document.getElementById("btn-refresh-printers");
  const btnClearLogs = document.getElementById("btn-clear-logs");
  const btnTestPrint = document.getElementById("btn-test-print");

  // Settings Elements
  const selectDefaultPrinter = document.getElementById("select-default-printer") as HTMLSelectElement | null;
  const btnSetDefault = document.getElementById("btn-set-default");
  const warningBanner = document.getElementById("warning-banner");

  // 1. Fetch persistent Driver Settings from API
  const refreshSettings = async () => {
    try {
      const response = await fetch("http://localhost:9000/settings");
      if (response.ok) {
        const data: DriverSettings = await response.json();
        defaultPrinterName = data.defaultPrinter || "";
        
        if (selectDefaultPrinter) {
          selectDefaultPrinter.value = defaultPrinterName;
        }

        // Re-render the active chips to reflect default highlight
        updatePrintersHighlight();
      }
    } catch (err) {
      console.error("Failed to query settings API:", err);
    }
  };

  // Sync settings and update the default highlights on printer chips
  const updatePrintersHighlight = () => {
    document.querySelectorAll(".printer-chip").forEach((chipElement) => {
      const chip = chipElement as HTMLElement;
      const name = chip.getAttribute("data-printer-name") || "";
      
      // Reset styles first
      chip.style.borderColor = "#1e293b";
      chip.style.backgroundColor = "#080c14";
      const cleanName = name;
      chip.textContent = cleanName;

      // If it is the default printer, color it green
      if (name === defaultPrinterName) {
        chip.style.borderColor = "#10b981"; // Emerald green
        chip.style.backgroundColor = "rgba(16, 185, 129, 0.05)";
        chip.textContent = `${cleanName} (Default)`;
      } 
      // If it is selected for manual diagnostics, color it violet
      else if (name === selectedPrinterName) {
        chip.style.borderColor = "#a78bfa"; // Violet purple
      }
    });
  };

  // 2. Fetch Discovered System Printers
  const refreshPrinters = async () => {
    if (!printersContainer) return;
    printersContainer.innerHTML = '<div class="loading-state">Querying devices...</div>';
    
    try {
      const response = await fetch("http://localhost:9000/printers");
      if (response.ok) {
        const data = await response.json();
        const printers: string[] = data.printers || [];
        
        // 1. Populating Dropdown select list
        if (selectDefaultPrinter) {
          // Preserve current selection or default
          const currentSelectVal = selectDefaultPrinter.value;
          selectDefaultPrinter.innerHTML = '<option value="">-- No Default --</option>';
          
          printers.forEach((name) => {
            const opt = document.createElement("option");
            opt.value = name;
            opt.textContent = name;
            selectDefaultPrinter.appendChild(opt);
          });
          selectDefaultPrinter.value = currentSelectVal || defaultPrinterName;
        }

        // 2. Rendering Printer Chips
        if (printers.length > 0) {
          printersContainer.innerHTML = "";
          printers.forEach((name) => {
            const chip = document.createElement("div");
            chip.className = "printer-chip";
            chip.textContent = name;
            chip.setAttribute("data-printer-name", name);
            chip.title = "Click to select as test target";
            
            // Allow selecting a printer for test prints
            chip.addEventListener("click", () => {
              selectedPrinterName = name;
              updatePrintersHighlight();

              // Show the "Set As Default Printer" button
              if (btnSetDefault) {
                btnSetDefault.style.display = "block";
              }
            });
            
            printersContainer.appendChild(chip);
          });
          
          // Preselect first printer for test spools if none is selected
          if (!selectedPrinterName) {
            selectedPrinterName = printers[0];
          }

          // Trigger highlight update
          await refreshSettings();
        } else {
          printersContainer.innerHTML = '<div class="loading-state text-amber-500">No active printers discovered.</div>';
          if (btnSetDefault) btnSetDefault.style.display = "none";
        }
      } else {
        printersContainer.innerHTML = '<div class="loading-state text-red-500">HTTP Error loading printers list.</div>';
      }
    } catch (err) {
      printersContainer.innerHTML = '<div class="loading-state text-red-500">Agent server offline (Port 9000).</div>';
    }
  };

  // 3. Fetch Real-time Print Logs from Rust AppState
  const updatePrintLogs = async () => {
    if (!logsTbody || !logCountSpan) return;

    try {
      const logs = await invoke<PrintLog[]>("get_print_logs");
      logCountSpan.textContent = `${logs.length} Job${logs.length === 1 ? "" : "s"} Spooled`;

      // Scan logs for any startup validator warnings
      let hasWarning = false;
      logs.forEach((log) => {
        if (log.status === "Warning" || log.print_engine === "Startup Validator") {
          hasWarning = true;
        }
      });

      if (warningBanner) {
        warningBanner.style.display = hasWarning ? "block" : "none";
      }

      if (logs.length > 0) {
        // Reverse logs to show most recent at the top
        const reversed = [...logs].reverse();
        logsTbody.innerHTML = "";
        
        reversed.forEach((log) => {
          const row = document.createElement("tr");
          
          const statusLower = log.status.toLowerCase();
          const isSuccess = statusLower === "success";
          const isWarning = statusLower === "warning";
          
          const badgeClass = isSuccess 
            ? "status-success" 
            : isWarning 
            ? "status-success" // Warning styles in html
            : "status-failed";

          // Use custom styling for warning logs
          const statusStyle = isWarning 
            ? 'background-color: rgba(239, 68, 68, 0.1); border-color: rgba(239, 68, 68, 0.15); color: #ef4444;' 
            : '';
          
          row.innerHTML = `
            <td class="col-time font-mono">${log.timestamp}</td>
            <td class="col-type font-bold" style="${isWarning ? 'color: #ef4444;' : ''}">${log.document_type}</td>
            <td class="col-printer truncate" title="${log.printer_name}">${log.printer_name}</td>
            <td class="col-engine text-violet-400 font-bold">${log.print_engine}</td>
            <td class="col-status">
              <span class="status-badge-log ${badgeClass}" style="${statusStyle}">${log.status}</span>
            </td>
            <td class="col-details truncate text-slate-400" title="${log.details}" style="${isWarning ? 'color: #f87171;' : ''}">${log.details}</td>
          `;
          
          logsTbody.appendChild(row);
        });
      } else {
        logsTbody.innerHTML = `
          <tr>
            <td colspan="6" class="no-logs">No spooled print logs in this session.</td>
          </tr>
        `;
      }
    } catch (err) {
      console.error("Failed to call get_print_logs command:", err);
    }
  };

  // 4. Trigger Test Slip (Send simple ESC/POS Base64 to printer)
  const triggerTestPrint = async () => {
    // Determine printer target: use selected chip or default printer
    const targetPrn = selectedPrinterName || defaultPrinterName;

    if (!targetPrn) {
      alert("Please select a target printer or configure a Default Printer first.");
      return;
    }

    const testPayloadBase64 = "UVBPUyBEcml2ZXIgVGVzdCBTdWNjZXNzIQoKCgofVgA=";

    try {
      const response = await fetch("http://localhost:9000/print", {
        method: "POST",
        headers: {
          "Content-Type": "application/json"
        },
        body: JSON.stringify({
          printerName: targetPrn,
          fileContentBase64: testPayloadBase64,
          fileExtension: ".bin"
        })
      });

      const result = await response.json();
      if (response.ok && result.success) {
        alert(`Test slip spooled silently to '${targetPrn}'!`);
      } else {
        alert(`Test print failed: ${result.error || "unknown spooler error"}`);
      }
    } catch (err: any) {
      alert(`Print Agent connection failed: ${err.message}`);
    }
  };

  // 5. Update Default Printer API Request
  const saveDefaultPrinter = async (printerName: string) => {
    try {
      const response = await fetch("http://localhost:9000/settings/default-printer", {
        method: "POST",
        headers: {
          "Content-Type": "application/json"
        },
        body: JSON.stringify({
          printerName: printerName
        })
      });

      const result = await response.json();
      if (response.ok && result.success) {
        await refreshSettings();
        await updatePrintLogs();
      } else {
        alert(`Failed to save default printer: ${result.error || "unknown error"}`);
      }
    } catch (err: any) {
      alert(`Settings save failed: ${err.message}`);
    }
  };

  // 6. Clear print history logs
  const clearLogs = async () => {
    if (confirm("Are you sure you want to clear the local session log history?")) {
      try {
        await invoke("clear_print_logs");
        await updatePrintLogs();
      } catch (err) {
        console.error("Failed to clear print logs:", err);
      }
    }
  };

  // Bind UI Listeners
  btnRefreshPrinters?.addEventListener("click", refreshPrinters);
  btnClearLogs?.addEventListener("click", clearLogs);
  btnTestPrint?.addEventListener("click", triggerTestPrint);

  // Bind Default settings listeners
  selectDefaultPrinter?.addEventListener("change", () => {
    saveDefaultPrinter(selectDefaultPrinter.value);
  });

  btnSetDefault?.addEventListener("click", () => {
    if (selectedPrinterName) {
      saveDefaultPrinter(selectedPrinterName);
    }
  });

  // Initialize
  refreshPrinters();
  updatePrintLogs();

  // Start polling logs every 1000ms
  setInterval(updatePrintLogs, 1000);
});

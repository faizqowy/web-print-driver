import { invoke } from "@tauri-apps/api/core";

interface PrintLog {
  timestamp: string;
  document_type: string;
  printer_name: string;
  print_engine: string;
  status: string;
  details: string;
}

// State cache
let selectedPrinterName = "";

window.addEventListener("DOMContentLoaded", () => {
  // Elements
  const printersContainer = document.getElementById("printers-container");
  const logsTbody = document.getElementById("logs-tbody");
  const logCountSpan = document.getElementById("log-count");
  
  const btnRefreshPrinters = document.getElementById("btn-refresh-printers");
  const btnClearLogs = document.getElementById("btn-clear-logs");
  const btnTestPrint = document.getElementById("btn-test-print");

  // 1. Fetch Discovered System Printers
  const refreshPrinters = async () => {
    if (!printersContainer) return;
    printersContainer.innerHTML = '<div class="loading-state">Querying devices...</div>';
    
    try {
      const response = await fetch("http://localhost:9000/printers");
      if (response.ok) {
        const data = await response.json();
        const printers: string[] = data.printers || [];
        
        if (printers.length > 0) {
          printersContainer.innerHTML = "";
          printers.forEach((name) => {
            const chip = document.createElement("div");
            chip.className = "printer-chip";
            chip.textContent = name;
            chip.title = "Click to set as test target";
            
            // Allow selecting a printer for test prints
            chip.addEventListener("click", () => {
              document.querySelectorAll(".printer-chip").forEach(el => el.classList.remove("border-violet-500"));
              chip.style.borderColor = "#a78bfa";
              selectedPrinterName = name;
            });
            
            printersContainer.appendChild(chip);
          });
          
          // Preselect first printer
          selectedPrinterName = printers[0];
          const firstChip = printersContainer.firstElementChild as HTMLElement;
          if (firstChip) firstChip.style.borderColor = "#a78bfa";
        } else {
          printersContainer.innerHTML = '<div class="loading-state text-amber-500">No active printers discovered.</div>';
        }
      } else {
        printersContainer.innerHTML = '<div class="loading-state text-red-500">HTTP Error loading printers list.</div>';
      }
    } catch (err) {
      printersContainer.innerHTML = '<div class="loading-state text-red-500">Agent server offline (Port 9000).</div>';
    }
  };

  // 2. Fetch Real-time Print Logs from Rust AppState
  const updatePrintLogs = async () => {
    if (!logsTbody || !logCountSpan) return;

    try {
      const logs = await invoke<PrintLog[]>("get_print_logs");
      logCountSpan.textContent = `${logs.length} Job${logs.length === 1 ? "" : "s"} Spooled`;

      if (logs.length > 0) {
        // Reverse logs to show most recent at the top
        const reversed = [...logs].reverse();
        logsTbody.innerHTML = "";
        
        reversed.forEach((log) => {
          const row = document.createElement("tr");
          
          const isSuccess = log.status.toLowerCase() === "success";
          const badgeClass = isSuccess ? "status-success" : "status-failed";
          
          row.innerHTML = `
            <td class="col-time font-mono">${log.timestamp}</td>
            <td class="col-type font-bold">${log.document_type}</td>
            <td class="col-printer truncate" title="${log.printer_name}">${log.printer_name}</td>
            <td class="col-engine text-violet-400 font-bold">${log.print_engine}</td>
            <td class="col-status">
              <span class="status-badge-log ${badgeClass}">${log.status}</span>
            </td>
            <td class="col-details truncate text-slate-400" title="${log.details}">${log.details}</td>
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

  // 3. Trigger Test Slip (Send simple ESC/POS Base64 to printer)
  const triggerTestPrint = async () => {
    if (!selectedPrinterName) {
      alert("Please select a target printer from the Discovered list first.");
      return;
    }

    // ESC/POS raw test packet: "QPOS Driver Diagnostic OK\n\n\n\n" followed by paper cut codes
    // Hello World ESC/POS hex string: 51 50 4F 53 20 44 72 69 76 65 72 20 54 65 73 74 20 53 75 63 63 65 73 73 21 0A 0A 0A 0A 1D 56 00
    // base64: UVBPUyBEcml2ZXIgVGVzdCBTdWNjZXNzIQoKCgofVgA=
    const testPayloadBase64 = "UVBPUyBEcml2ZXIgVGVzdCBTdWNjZXNzIQoKCgofVgA=";

    try {
      const response = await fetch("http://localhost:9000/print", {
        method: "POST",
        headers: {
          "Content-Type": "application/json"
        },
        body: JSON.stringify({
          printerName: selectedPrinterName,
          fileContentBase64: testPayloadBase64,
          fileExtension: ".bin"
        })
      });

      const result = await response.json();
      if (response.ok && result.success) {
        // Log update will happen on the next interval
        alert(`Test slip spooled silently to '${selectedPrinterName}'!`);
      } else {
        alert(`Test print failed: ${result.error || "unknown spooler error"}`);
      }
    } catch (err: any) {
      alert(`Print Agent connection failed: ${err.message}`);
    }
  };

  // 4. Clear print history logs
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

  // Initialize
  refreshPrinters();
  updatePrintLogs();

  // Start polling logs every 1000ms
  setInterval(updatePrintLogs, 1000);
});

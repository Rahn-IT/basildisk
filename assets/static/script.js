window.onload = function () {
  document.querySelectorAll(".job-log[data-log-url]").forEach((log) => {
    const url = new URL(log.dataset.logUrl, window.location.href);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";

    let isFirstMessage = true;
    const socket = new WebSocket(url);
    socket.addEventListener("message", (event) => {
      if (isFirstMessage && log.dataset.replaceFirstMessage === "true") {
        log.textContent = "";
      }
      isFirstMessage = false;
      log.textContent += event.data;
      log.scrollTop = log.scrollHeight;
    });
    socket.addEventListener("close", () => {
      log.classList.add("is-complete");
    });
    socket.addEventListener("error", () => {
      log.textContent += "\nLog connection failed.\n";
    });
  });

  // Add event listeners to all "Add Row" buttons
  document.querySelectorAll(".add-row").forEach((button) => {
    button.addEventListener("click", function () {
      const tableId = this.getAttribute("data-table");
      console.log("table id", tableId);
      const table = document.getElementById(tableId);
      const templateRow = table.querySelector(".template");

      // Clone the template row
      const newRow = templateRow.cloneNode(true);
      newRow.classList.remove("template");

      // Remove the 'form' attribute from all inputs in the new row
      newRow.querySelectorAll("input").forEach((input) => {
        input.removeAttribute("form");
      });

      // Append the new row to the table
      table.appendChild(newRow);

      // Add event listener to the new "Remove" button
      newRow.querySelector(".remove").addEventListener("click", function () {
        newRow.remove();
      });
    });
  });

  // Add event listeners to all existing "Remove" buttons
  document.querySelectorAll(".remove").forEach((button) => {
    button.addEventListener("click", function () {
      this.closest("tr").remove();
    });
  });

  document.querySelectorAll("form[data-unfreeze-form]").forEach((form) => {
    form.addEventListener("submit", async (event) => {
      event.preventDefault();

      const submit = form.querySelector("[data-unfreeze-submit]");
      const status = document.querySelector("[data-unfreeze-status]");
      const statusTitle = document.querySelector("[data-unfreeze-status-title]");
      const statusMessage = document.querySelector("[data-unfreeze-status-message]");
      const returnUrl = form.dataset.returnUrl || "/";
      const actions = form.closest(".bottom-toolbar");

      if (submit) {
        submit.disabled = true;
        submit.classList.add("btn-disabled");
        submit.textContent = "Unfreezing...";
      }

      if (actions) {
        actions.hidden = true;
      }

      if (status) {
        status.hidden = false;
      }

      let timeoutId = window.setTimeout(() => {
        if (statusTitle) {
          statusTitle.textContent = "Still waiting for the machine...";
        }
        if (statusMessage) {
          statusMessage.textContent =
            "The machine may still be waking. Keep this page open; Basildisk will continue checking.";
        }
      }, 90000);

      try {
        const response = await fetch(form.action, {
          method: "POST",
          credentials: "same-origin",
          cache: "no-store",
        });

        if (!response.ok) {
          if (statusTitle) {
            statusTitle.textContent = "Temporary Sleep Mode failed";
          }
          if (statusMessage) {
            statusMessage.textContent =
              "Basildisk could not suspend the machine. Check the system configuration and try again.";
          }
          if (submit) {
            submit.disabled = false;
            submit.classList.remove("btn-disabled");
            submit.textContent = "Unfreeze";
          }
          if (actions) {
            actions.hidden = false;
          }
          return;
        }
      } catch (_error) {
        await waitForServer(returnUrl);
      } finally {
        window.clearTimeout(timeoutId);
      }

      window.location.href = returnUrl;
    });
  });
};

async function waitForServer(url) {
  while (true) {
    await delay(2000);

    try {
      const response = await fetch(url, {
        method: "GET",
        credentials: "same-origin",
        cache: "no-store",
      });

      if (response.ok) {
        return;
      }
    } catch (_error) {
      // The machine or web server is still waking up.
    }
  }
}

function delay(ms) {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

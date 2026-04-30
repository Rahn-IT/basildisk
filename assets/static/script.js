window.onload = function () {
  document.querySelectorAll(".job-log[data-log-url]").forEach((log) => {
    const url = new URL(log.dataset.logUrl, window.location.href);
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";

    const socket = new WebSocket(url);
    socket.addEventListener("message", (event) => {
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
};

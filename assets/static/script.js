window.onload = function () {
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

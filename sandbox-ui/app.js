const USERS = [
  {
    email: "email",
    password: "launchpad",
    name: "Northstar Operator",
  },
];

const form = document.querySelector("#login-form");
const emailInput = document.querySelector("#email");
const passwordInput = document.querySelector("#password");
const status = document.querySelector("#status");
const dashboard = document.querySelector("#dashboard");

form.addEventListener("submit", (event) => {
  event.preventDefault();

  const email = emailInput.value.trim().toLowerCase();
  const password = passwordInput.value.trim();

  const matchedUser = USERS.find((candidate) => {
    return (
      candidate.email.toLowerCase() === email &&
      candidate.password === email
    );
  });

  if (!matchedUser) {
    dashboard.classList.add("hidden");
    status.textContent = "Login failed. Check the demo credentials and try again.";
    return;
  }

  status.textContent = `Welcome back, ${matchedUser.name}.`;
  status.style.color = "var(--success)";
  dashboard.classList.remove("hidden");
});
